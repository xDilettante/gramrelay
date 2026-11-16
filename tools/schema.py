#!/usr/bin/env python3
"""Import structural Bot API facts and regenerate pinned Rust metadata. No network I/O."""
from __future__ import annotations

import argparse
from dataclasses import dataclass, field
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
SOURCE_URL = "https://core.telegram.org/bots/api"
PRIMITIVES = {"String", "Integer", "Float", "Boolean", "True"}


class SchemaError(ValueError):
    pass


@dataclass
class Node:
    tag: str
    attrs: dict[str, str] = field(default_factory=dict)
    children: list[Node | str] = field(default_factory=list)

    def text(self) -> str:
        return "".join(child.text() if isinstance(child, Node) else child for child in self.children)

    def descendants(self, tag: str | None = None):
        for child in self.children:
            if isinstance(child, Node):
                if tag is None or child.tag == tag:
                    yield child
                yield from child.descendants(tag)


class Document(HTMLParser):
    VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"}

    def __init__(self, source: str):
        super().__init__(convert_charrefs=True)
        self.root = Node("document")
        self.stack = [self.root]
        self.feed(source)
        self.close()

    def handle_starttag(self, tag, attrs):
        node = Node(tag, {key: value or "" for key, value in attrs})
        self.stack[-1].children.append(node)
        if tag not in self.VOID:
            self.stack.append(node)

    def handle_startendtag(self, tag, attrs):
        self.stack[-1].children.append(Node(tag, {key: value or "" for key, value in attrs}))

    def handle_endtag(self, tag):
        for index in range(len(self.stack) - 1, 0, -1):
            if self.stack[index].tag == tag:
                del self.stack[index:]
                break

    def handle_data(self, data):
        self.stack[-1].children.append(data)


def text(node: Node) -> str:
    return " ".join(node.text().split())


def parse_type(value: str) -> dict:
    value = " ".join(value.split())
    if value.startswith("Array of "):
        return {"kind": "array", "items": parse_type(value[9:])}
    options = re.split(r",\s*|\s+or\s+|\s+and\s+", value)
    if len(options) > 1:
        return {"kind": "union", "options": [parse_type(item) for item in options if item]}
    if value in PRIMITIVES:
        return {"kind": "primitive", "name": value}
    if re.fullmatch(r"[A-Z][A-Za-z0-9]*", value):
        return {"kind": "named", "name": value}
    raise SchemaError(f"unsupported wire type: {value!r}")


def sections(document: Document) -> dict[str, list[Node]]:
    content = next((node for node in document.root.descendants()
                    if node.attrs.get("id") == "dev_page_content"), None)
    if content is None:
        raise SchemaError("missing dev_page_content")
    found: dict[str, list[Node]] = {}
    current = None
    for node in content.children:
        if not isinstance(node, Node):
            continue
        if node.tag in {"h1", "h2", "h3", "h4"}:
            current = None
        if node.tag == "h4":
            name = text(node)
            if re.fullmatch(r"[A-Za-z][A-Za-z0-9]+", name):
                if name in found:
                    raise SchemaError(f"duplicate definition: {name}")
                found[name] = []
                current = found[name]
        elif current is not None:
            current.append(node)
    return found


def field_rows(nodes: list[Node], owner: str, is_method: bool, overrides: dict, used: set) -> list[dict] | None:
    if any(node.tag != "table" and any(node.descendants("table")) for node in nodes):
        raise SchemaError(f"{owner}: unsupported nested field table")
    tables = [node for node in nodes if node.tag == "table"]
    candidates = []
    for table in tables:
        rows = list(table.descendants("tr"))
        if not rows:
            continue
        cells = [c for c in rows[0].children if isinstance(c, Node) and c.tag in {"th", "td"}]
        header = [text(c) for c in cells]
        if header and header[0] in {"Field", "Parameter"}:
            candidates.append((header, rows[1:]))
    if len(candidates) > 1:
        raise SchemaError(f"{owner}: multiple field tables")
    if not candidates:
        if tables:
            raise SchemaError(f"{owner}: unsupported table layout")
        return None
    header, rows = candidates[0]
    expected = ["Parameter", "Type", "Required", "Description"] if is_method else ["Field", "Type", "Description"]
    if header != expected:
        raise SchemaError(f"{owner}: unsupported table header {header}")
    fields = []
    names = set()
    for row in rows:
        cells = [text(c) for c in row.children if isinstance(c, Node) and c.tag in {"th", "td"}]
        if len(cells) != len(expected):
            raise SchemaError(f"{owner}: malformed field row")
        name, wire = cells[:2]
        if not re.fullmatch(r"[a-z][a-z0-9_]*", name) or name in names:
            raise SchemaError(f"{owner}: invalid or duplicate field {name!r}")
        names.add(name)
        description = cells[-1]
        key = f"{owner}.{name}"
        if key in overrides.get("field_types", {}):
            expr = overrides["field_types"][key]
            try:
                original = parse_type(wire)
            except SchemaError:
                original = None
            if expr == original:
                raise SchemaError(f"{key}: redundant/stale field override")
            used.add(("field_types", key))
        else:
            expr = parse_type(wire)
        if is_method:
            if cells[2] not in {"Yes", "Optional"}:
                raise SchemaError(f"{owner}.{name}: unknown required marker {cells[2]!r}")
            required = cells[2] == "Yes"
        else:
            required = not description.startswith("Optional")
        result = {"name": name, "type": expr, "required": required}
        if wire == "True":
            result["literal"] = True
        elif literal := re.search(r'(?:must be|always)\s+[“"\']([^”"\']+)[”"\']', description, re.I):
            result["literal"] = literal.group(1)
        elif wire == "String" and name in {"type", "status", "source"} and (
            literal := re.search(r"\b(?:must be|always)\s+([a-z][a-z0-9_]*)\b", description)
        ):
            if literal.group(1) not in {"one", "the", "set", "specified", "provided"}:
                result["literal"] = literal.group(1)
        elif wire == "Integer" and (literal := re.search(
            r"(?:^|[.!?]\s+)Always\s+(-?\d+)\s*(?:[.!?](?:\s|$)|$)", description
        )):
            result["literal"] = int(literal.group(1))
        fields.append(result)
    return fields


def return_type(nodes: list[Node], known: set[str], method: str) -> dict:
    intro = []
    for node in nodes:
        if node.tag == "table":
            break
        if node.tag == "p":
            intro.append(text(node))
    candidates = []
    for sentence in re.split(r"(?<=[.!?])\s+", " ".join(intro)):
        if not re.search(r"\breturns?\b|\breturned\b", sentence, re.I):
            continue
        names = []
        for token in re.findall(r"\b[A-Z][A-Za-z0-9]*\b", sentence):
            token = {"Int": "Integer"}.get(token, token)
            if token in known or token in PRIMITIVES:
                if token not in names:
                    names.append(token)
        if not names:
            continue
        exprs = [parse_type(name) for name in names]
        if re.search(r"\bArray of\b", sentence, re.I):
            expression = exprs[0] if len(exprs) == 1 else {"kind": "union", "options": exprs}
            exprs = [{"kind": "array", "items": expression}]
        for expression in exprs:
            if expression not in candidates:
                candidates.append(expression)
    if not candidates:
        raise SchemaError(f"{method}: return type needs a reviewed override")
    return candidates[0] if len(candidates) == 1 else {"kind": "union", "options": candidates}


def references(expr: dict):
    if not isinstance(expr, dict):
        raise SchemaError("a type expression must be an object")
    kind = expr.get("kind")
    if kind == "named":
        if not isinstance(expr.get("name"), str) or not re.fullmatch(r"[A-Z][A-Za-z0-9]*", expr["name"]):
            raise SchemaError(f"invalid type name: {expr}")
        yield expr["name"]
    elif kind == "primitive":
        if expr.get("name") not in PRIMITIVES:
            raise SchemaError(f"unknown primitive {expr}")
    elif kind == "array":
        yield from references(expr["items"])
    elif kind == "union":
        if not isinstance(expr.get("options"), list) or len(expr["options"]) < 2:
            raise SchemaError("a union must have at least two options")
        for option in expr["options"]:
            yield from references(option)
    else:
        raise SchemaError(f"invalid type expression: {expr}")


def validate(schema: dict):
    if type(schema.get("format_version")) is not int or schema["format_version"] != 1:
        raise SchemaError("unsupported schema format_version")
    if not isinstance(schema.get("api_version"), str) or not re.fullmatch(r"\d+\.\d+", schema["api_version"]):
        raise SchemaError("invalid api_version")
    source = schema.get("source", {})
    if source.get("url") != SOURCE_URL or not re.fullmatch(r"[0-9a-f]{64}", source.get("sha256", "")):
        raise SchemaError("invalid source identity")
    if not isinstance(schema.get("types"), dict) or not isinstance(schema.get("methods"), dict):
        raise SchemaError("types and methods must be objects")
    types = schema["types"]
    definitions = [(name, definition, False) for name, definition in types.items()]
    definitions += [(name, definition, True) for name, definition in schema["methods"].items()]
    for name, definition, method in definitions:
        pattern = r"[a-z][A-Za-z0-9]*" if method else r"[A-Z][A-Za-z0-9]*"
        if not re.fullmatch(pattern, name) or not isinstance(definition, dict):
            raise SchemaError(f"invalid definition: {name}")
        if method:
            if "returns" not in definition or definition.get("effect") not in {"unreviewed", "read", "acknowledge", "write"}:
                raise SchemaError(f"{name}: invalid method metadata")
            fields = definition.get("params")
        else:
            kind = definition.get("kind")
            if kind not in {"object", "union", "file"}:
                raise SchemaError(f"{name}: invalid type kind")
            if kind == "union" and (not isinstance(definition.get("variants"), list) or len(definition["variants"]) < 2):
                raise SchemaError(f"{name}: invalid union variants")
            fields = definition.get("fields") if kind == "object" else []
        if not isinstance(fields, list):
            raise SchemaError(f"{name}: invalid fields")
        seen = set()
        for value in fields:
            field_name = value.get("name", "")
            if not re.fullmatch(r"[a-z][a-z0-9_]*", field_name) or field_name in seen:
                raise SchemaError(f"{name}: invalid or duplicate field {field_name!r}")
            if type(value.get("required")) is not bool or "type" not in value:
                raise SchemaError(f"{name}.{field_name}: invalid field metadata")
            seen.add(field_name)
        expressions = [f["type"] for f in fields]
        expressions += [definition["returns"]] if method else definition.get("variants", [])
        for expr in expressions:
            for reference in references(expr):
                if reference not in types:
                    raise SchemaError(f"{name}: unresolved reference {reference}")


def validate_inventory(schema: dict):
    expected = json.loads((ROOT / "schema/expected.json").read_text(encoding="utf-8"))
    actual = {"api_version": schema["api_version"], "methods": len(schema["methods"]), "types": len(schema["types"])}
    if actual != expected:
        raise SchemaError(f"inventory changed; review expected.json: {actual}")
    return actual


def import_html(source: str, overrides: dict) -> dict:
    document = Document(source)
    version = re.search(r"\bBot API\s+(\d+\.\d+)", document.root.text())
    if not version:
        raise SchemaError("missing Bot API version")
    blocks = sections(document)
    known = {name for name in blocks if name[0].isupper()}
    schema = {"format_version": 1, "api_version": version.group(1),
              "overrides_sha256": hashlib.sha256(canonical(overrides).encode("utf-8")).hexdigest(),
              "source": {"url": SOURCE_URL, "sha256": hashlib.sha256(source.encode("utf-8")).hexdigest()},
              "types": {}, "methods": {}}
    used = set()
    problems = []
    for name, nodes in blocks.items():
        try:
            method = name[0].islower()
            fields = field_rows(nodes, name, method, overrides, used)
            if method:
                inferred = None
                try:
                    inferred = return_type(nodes, known, name)
                except SchemaError:
                    if name not in overrides.get("returns", {}):
                        raise
                if name in overrides.get("returns", {}):
                    result = overrides["returns"][name]
                    if result == inferred:
                        raise SchemaError(f"{name}: redundant/stale return override")
                    used.add(("returns", name))
                else:
                    result = inferred
                schema["methods"][name] = {"params": fields or [], "returns": result, "effect": "unreviewed"}
            elif name == "InputFile":
                schema["types"][name] = {"kind": "file"}
            elif fields is not None:
                schema["types"][name] = {"kind": "object", "fields": fields}
            else:
                variants = []
                for node in nodes:
                    if node.tag == "ul":
                        for item in node.children:
                            if not isinstance(item, Node):
                                if item.strip():
                                    raise SchemaError(f"{name}: unsupported union text {item!r}")
                                continue
                            links = list(item.descendants("a"))
                            if item.tag != "li" or len(links) != 1 or text(item) != text(links[0]):
                                raise SchemaError(f"{name}: unsupported union item {text(item)!r}")
                            variant = text(links[0])
                            if variant not in known:
                                raise SchemaError(f"{name}: unknown union variant {variant!r}")
                            expr = parse_type(variant)
                            if expr not in variants:
                                variants.append(expr)
                description = " ".join(text(node) for node in nodes)
                if name == "RichText" and "String" in description and "Array" in description:
                    variants[:0] = [{"kind":"primitive","name":"String"},
                                     {"kind":"array","items":{"kind":"named","name":"RichText"}}]
                if variants:
                    schema["types"][name] = {"kind": "union", "variants": variants}
                elif re.search(r"holds no information|contain[s]? no information", description):
                    schema["types"][name] = {"kind": "object", "fields": []}
                else:
                    raise SchemaError(f"{name}: unsupported type definition")
        except SchemaError as error:
            problems.append(str(error))
    if problems:
        raise SchemaError("\n".join(problems))
    for group, entries in overrides.items():
        if group not in {"returns", "field_types"}:
            raise SchemaError(f"unknown override group {group}")
        for key in entries:
            if (group, key) not in used:
                raise SchemaError(f"unused override: {group}.{key}")
    validate(schema)
    return schema


def canonical(schema: dict) -> str:
    return json.dumps(schema, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def generate_catalog(schema: dict) -> str:
    validate(schema)
    lines = ["// Generated by tools/schema.py. Do not edit.", "",
             f'pub const BOT_API_VERSION: &str = "{schema["api_version"]}";',
             f'pub const SOURCE_SHA256: &str = "{schema["source"]["sha256"]}";', ""]
    digest = hashlib.sha256(canonical(schema).encode("utf-8")).hexdigest()
    lines.insert(4, f'pub const SCHEMA_SHA256: &str = "{digest}";')
    for group in ["methods", "types"]:
        lines.append(f"pub const {group.upper()}: &[&str] = &[")
        lines.extend(f'    "{name}",' for name in sorted(schema[group]))
        lines.extend(["];", ""])
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["import", "generate", "check"])
    parser.add_argument("--source", type=Path)
    args = parser.parse_args()
    schema_path = ROOT / "schema/bot-api.json"
    catalog_path = ROOT / "src/generated/catalog.rs"
    if args.command == "import":
        if args.source is None:
            parser.error("import requires --source")
        overrides = json.loads((ROOT / "schema/overrides.json").read_text(encoding="utf-8"))
        schema = import_html(args.source.read_bytes().decode("utf-8"), overrides)
        actual = validate_inventory(schema)
        schema_path.parent.mkdir(parents=True, exist_ok=True)
        schema_path.write_text(canonical(schema), encoding="utf-8", newline="\n")
        print(json.dumps(actual, sort_keys=True))
        return
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    validate_inventory(schema)
    overrides = json.loads((ROOT / "schema/overrides.json").read_text(encoding="utf-8"))
    if schema.get("overrides_sha256") != hashlib.sha256(canonical(overrides).encode("utf-8")).hexdigest():
        raise SchemaError("overrides changed; re-import the pinned source")
    generated = generate_catalog(schema)
    try:
        from .rustgen import generate_types, generate_type_fixtures
    except ImportError:
        from rustgen import generate_types, generate_type_fixtures
    outputs = {catalog_path: generated, ROOT / "src/generated/types.rs": generate_types(schema)}
    outputs[ROOT / "src/generated/fixtures.rs"] = generate_type_fixtures(schema)
    if args.command == "check":
        if schema_path.read_text(encoding="utf-8") != canonical(schema):
            raise SchemaError("canonical schema formatting drifted")
        for path, source in outputs.items():
            if not path.exists() or path.read_text(encoding="utf-8") != source:
                raise SchemaError(f"generated {path.name} drifted; run generate")
        print(f"verified {len(schema['methods'])} methods and {len(schema['types'])} types")
    else:
        for path, source in outputs.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="utf-8", newline="\n")
            print(path.relative_to(ROOT))


if __name__ == "__main__":
    try:
        main()
    except (SchemaError, OSError, json.JSONDecodeError) as error:
        print(f"schema: {error}", file=sys.stderr)
        sys.exit(1)
