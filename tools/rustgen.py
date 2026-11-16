"""Deterministic Rust wire types from the reviewed structural schema (developer-only)."""

import copy
import json

try:
    from .schema import SchemaError, validate
except ImportError:
    from schema import SchemaError, validate


def quoted(value):
    return json.dumps(value, ensure_ascii=False)


def pascal(value):
    return ''.join(part[:1].upper() + part[1:] for part in value.split('_'))


def field_name(value):
    if value == 'type':
        return 'kind'
    if value in {'ref', 'match', 'self', 'super', 'crate', 'move', 'async', 'await', 'loop', 'fn', 'mod'}:
        return 'r#' + value
    return value


PRIMITIVES = {'String': 'String', 'Integer': 'i64', 'Float': 'f64',
              'Boolean': 'bool', 'True': 'crate::api::True'}
CHAT_IDS = {'chat_id', 'linked_chat_id', 'migrate_from_chat_id', 'migrate_to_chat_id', 'user_chat_id'}
USER_IDS = {'user_id', 'receiver_user_id'}
MESSAGE_IDS = {'message_id', 'giveaway_message_id'}
FILE_IDS = {'file_id', 'small_file_id', 'big_file_id', 'audio_file_id', 'document_file_id',
            'gif_file_id', 'mpeg4_file_id', 'photo_file_id', 'sticker_file_id', 'video_file_id', 'voice_file_id'}


class Model:
    def __init__(self, schema):
        validate(schema)
        self.types = copy.deepcopy(schema['types'])
        self.literals = {}
        self.boxed = set()
        for owner, item in list(self.types.items()):
            for field in item.get('fields', []):
                field['type'] = self.normalize(field['type'], owner + pascal(field['name']))
                if 'literal' in field:
                    literal = field['literal']
                    if isinstance(literal, str):
                        if field['type'] != {'kind': 'primitive', 'name': 'String'}:
                            raise SchemaError(f'{owner}.{field["name"]}: non-string literal type')
                        name = owner + pascal(field['name'])
                        if name in self.types or name in self.literals:
                            raise SchemaError(f'duplicate literal type {name}')
                        self.literals[name] = literal
                    elif literal is True:
                        if field['type'].get('name') not in {'True', 'Boolean'}:
                            raise SchemaError('true literal on a non-boolean field')
                    elif type(literal) is int and literal == 0:
                        if field['type'] != {'kind': 'primitive', 'name': 'Integer'}:
                            raise SchemaError('zero literal on a non-integer field')
                    else:
                        raise SchemaError(f'{owner}.{field["name"]}: unsupported literal {literal!r}')
        # Keep enum storage bounded regardless of an individual DTO's size.
        self.boxed = {(owner, variant['name']) for owner, item in self.types.items()
                      if item['kind'] == 'union' for variant in item['variants']
                      if variant['kind'] == 'named'}
        self.cut_cycles()

    def normalize(self, expr, hint):
        if expr['kind'] == 'array':
            return {'kind': 'array', 'items': self.normalize(expr['items'], hint + 'Item')}
        if expr['kind'] != 'union':
            return expr
        if {json.dumps(x, sort_keys=True) for x in expr['options']} == {
            json.dumps({'kind': 'primitive', 'name': n}, sort_keys=True) for n in ['Integer', 'String']
        }:
            return {'kind': 'external', 'name': 'crate::api::ChatTarget'}
        # Wire InputFile is a string reference; uploaded bytes live in the client layer.
        if all(x in [{'kind': 'named', 'name': 'InputFile'}, {'kind': 'primitive', 'name': 'String'}]
               for x in expr['options']):
            return {'kind': 'named', 'name': 'InputFile'}
        if hint in self.types:
            raise SchemaError(f'inline union name collides: {hint}')
        self.types[hint] = {'kind': 'union', 'variants': [
            self.normalize(option, hint + str(index)) for index, option in enumerate(expr['options'])]}
        return {'kind': 'named', 'name': hint}

    def direct_edges(self, name):
        item = self.types[name]
        expressions = ([field['type'] for field in item['fields']
                        if field['required'] or not self.optional_object(field)] if item['kind'] == 'object'
                       else item.get('variants', []))
        return sorted({expr['name'] for expr in expressions if expr['kind'] == 'named'
                       and (name, expr['name']) not in self.boxed})

    def cut_cycles(self):
        active, done = set(), set()

        def visit(name):
            if name in done:
                return
            active.add(name)
            for target in self.direct_edges(name):
                if target in active:
                    self.boxed.add((name, target))
                else:
                    visit(target)
            active.remove(name)
            done.add(name)

        for name in sorted(self.types):
            visit(name)

    def rust_type(self, expr, owner=None):
        kind = expr['kind']
        if kind == 'primitive':
            return PRIMITIVES[expr['name']]
        if kind == 'external':
            return expr['name']
        if kind == 'array':
            return f'Vec<{self.rust_type(expr["items"])}>'
        if kind == 'named':
            name = expr['name']
            return f'Box<{name}>' if (owner, name) in self.boxed else name
        raise SchemaError(f'unhandled expression {expr}')

    def field_type(self, owner, field):
        literal = field.get('literal')
        if isinstance(literal, str):
            return owner + pascal(field['name'])
        if literal is True:
            return 'crate::api::True'
        if type(literal) is int and literal == 0:
            return 'crate::api::Zero'
        expr, name = field['type'], field['name']
        if expr == {'kind': 'primitive', 'name': 'Integer'}:
            if name in CHAT_IDS or (owner in {'Chat', 'ChatFullInfo'} and name == 'id'):
                return 'crate::ChatId'
            if name in USER_IDS or (owner == 'User' and name == 'id'):
                return 'crate::UserId'
            if name in MESSAGE_IDS:
                return 'crate::MessageId'
        if expr == {'kind': 'primitive', 'name': 'String'}:
            if name in FILE_IDS:
                return 'crate::FileId'
            if owner == 'CallbackQuery' and name == 'id':
                return 'crate::CallbackQueryId'
        if name == 'message_ids' and expr == {'kind': 'array', 'items': {'kind': 'primitive', 'name': 'Integer'}}:
            return 'Vec<crate::MessageId>'
        if self.optional_object(field):
            return f'Box<{self.rust_type(expr)}>'
        return self.rust_type(expr, owner)

    def optional_object(self, field):
        expr = field['type']
        return (not field['required'] and expr['kind'] == 'named'
                and self.types[expr['name']]['kind'] == 'object')

    def object_source(self, name, item):
        fields = item['fields']
        lines = [f'/// Telegram [`{name}`](https://core.telegram.org/bots/api#{name.lower()}).',
                 '#[derive(Debug, Clone, Serialize, Deserialize)]', '#[non_exhaustive]']
        if name == 'Update':
            lines.append('#[serde(try_from = "UpdateFields")]')
        lines.append(f'pub struct {name} {{')
        lines.extend(self.field_source(name, field) for field in fields)
        lines.extend(['    #[serde(flatten)]', '    unknown_fields: crate::wire::UnknownFields,', '}'])
        if name == 'Update':
            lines += ['#[derive(Deserialize)]', 'struct UpdateFields {']
            lines.extend(self.field_source(name, field, public=False) for field in fields)
            lines += ['    #[serde(flatten)]', '    unknown_fields: crate::wire::UnknownFields,', '}',
                      'impl TryFrom<UpdateFields> for Update {', "    type Error = &'static str;",
                      '    fn try_from(wire: UpdateFields) -> Result<Self, Self::Error> {',
                      '        let count = [']
            lines += [f'            wire.{field_name(f["name"])}.is_some(),'
                      for f in fields if not f['required']]
            lines += ['        ].into_iter().filter(|present| *present).count();', '        if count > 1 {',
                      '            return Err("an update cannot contain multiple known event kinds");',
                      '        }', '        Ok(Self {']
            lines += [f'            {field_name(f["name"])}: wire.{field_name(f["name"])},' for f in fields]
            lines += ['            unknown_fields: wire.unknown_fields,', '        })', '    }', '}']
        required = [field for field in fields if field['required'] and 'literal' not in field]
        lines += [f'impl {name} {{', '    #[allow(clippy::too_many_arguments)]']
        args = ', '.join(f'{field_name(f["name"])}: {self.field_type(name, f)}' for f in required)
        lines += [f'    pub fn new({args}) -> Self {{', '        Self {']
        for f in fields:
            key = field_name(f['name'])
            value = ('Default::default()' if 'literal' in f and f['required'] else
                     key if f['required'] else 'None')
            lines.append(f'            {key},' if value == key else f'            {key}: {value},')
        lines += ['            unknown_fields: Default::default(),', '        }', '    }',
                  '    pub fn unknown_fields(&self) -> &std::collections::BTreeMap<String, serde_json::Value> {',
                  '        self.unknown_fields.as_map()', '    }']
        for f in fields:
            if not f['required']:
                key = field_name(f['name'])
                field_type = self.field_type(name, f)
                boxed = self.optional_object(f)
                input_type = self.rust_type(f['type']) if boxed else field_type
                value = 'Box::new(value)' if boxed else 'value'
                lines += [f'    pub fn {key}(mut self, value: {input_type}) -> Self {{',
                          f'        self.{key} = Some({value});', '        self', '    }']
        lines += ['}']
        if not required:
            lines += [f'impl Default for {name} {{', '    fn default() -> Self { Self::new() }', '}']
        return '\n'.join(lines)

    def field_source(self, owner, field, public=True):
        name = field_name(field['name'])
        lines = []
        if name != field['name']:
            lines.append(f'    #[serde(rename = {quoted(field["name"])})]')
        field_type = self.field_type(owner, field)
        if not field['required']:
            lines.append('    #[serde(default, skip_serializing_if = "Option::is_none")]')
            field_type = f'Option<{field_type}>'
        lines.append(f'    {"pub " if public else ""}{name}: {field_type},')
        return '\n'.join(lines)

    def variant_name(self, expr):
        if expr['kind'] in {'named', 'primitive'}:
            return expr['name']
        if expr['kind'] == 'array':
            return 'Array'
        raise SchemaError(f'unsupported union variant {expr}')

    def decode(self, owner, variant, expression='value'):
        name = self.variant_name(variant)
        value = f'serde_json::from_value({expression}).map(Self::{name}).map_err(D::Error::custom)'
        return value

    def select_objects(self, owner, variants, indent):
        lines = [indent + 'let shapes = &[']
        for expr in variants:
            fields = self.types[expr['name']]['fields']
            required = ', '.join(quoted(f['name']) for f in fields if f['required'])
            allowed = ', '.join(quoted(f['name']) for f in fields)
            lines += [indent + f'    crate::wire::ObjectShape {{ required: &[{required}], allowed: &[{allowed}] }},']
        lines += [indent + '];', indent + 'match crate::wire::select_shape(&value, shapes).map_err(D::Error::custom)? {']
        lines += [indent + f'    {index} => {self.decode(owner, variant)},' for index, variant in enumerate(variants)]
        lines += [indent + '    _ => unreachable!("shape index is bounded by generated variants"),', indent + '}']
        return lines

    def union_source(self, name, item):
        variants = item['variants']
        named = [v for v in variants if v['kind'] == 'named']
        if any(self.types[v['name']]['kind'] != 'object' for v in named):
            raise SchemaError(f'{name}: non-object named union branch needs review')
        tags = []
        for tag in ['type', 'status', 'source']:
            if named and all(any(f['name'] == tag and f['required'] and isinstance(f.get('literal'), str)
                                 for f in self.types[v['name']]['fields']) for v in named):
                tags.append(tag)
        if len(tags) > 1:
            raise SchemaError(f'{name}: ambiguous discriminator fields')
        tag = tags[0] if tags else None
        lines = ['#[derive(Debug, Clone, Serialize)]', '#[serde(untagged)]', '#[non_exhaustive]', f'pub enum {name} {{']
        lines += [f'    {self.variant_name(v)}({self.rust_type(v, name)}),' for v in variants]
        if tag:
            lines.append('    Unknown(crate::api::UnknownVariant),')
        lines += ['}', f"impl<'de> Deserialize<'de> for {name} {{",
                  "    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {",
                  '        let value = serde_json::Value::deserialize(deserializer)?;']
        for variant in variants:
            if variant['kind'] != 'named':
                check = {'String': 'is_string', 'Integer': 'is_i64', 'Float': 'is_number',
                         'Boolean': 'is_boolean', 'True': 'is_boolean'}.get(variant.get('name'), 'is_array')
                lines += [f'        if value.{check}() {{', f'            return {self.decode(name, variant)};', '        }']
        if tag:
            groups = {}
            for v in named:
                literal = next(f['literal'] for f in self.types[v['name']]['fields'] if f['name'] == tag)
                groups.setdefault(literal, []).append(v)
            lines += [f'        match value.get({quoted(tag)}).and_then(serde_json::Value::as_str) {{']
            for literal, group in sorted(groups.items()):
                if len(group) == 1:
                    lines += [f'            Some({quoted(literal)}) => {self.decode(name, group[0])},']
                else:
                    lines += [f'            Some({quoted(literal)}) => {{']
                    lines += self.select_objects(name, group, '                ')
                    lines += ['            }']
            lines += ['            Some(_) => Ok(Self::Unknown(crate::api::UnknownVariant::new(value))),',
                      f'            None => Err(D::Error::custom("missing or invalid {name} discriminator")),', '        }']
        elif name == 'MaybeInaccessibleMessage':
            if [v['name'] for v in variants] != ['Message', 'InaccessibleMessage']:
                raise SchemaError('MaybeInaccessibleMessage variants changed; review date selection')
            lines += ['        match value.get("date").and_then(serde_json::Value::as_i64) {',
                      f'            Some(0) => {self.decode(name, named[1])},',
                      f'            Some(_) => {self.decode(name, named[0])},',
                      '            None => Err(D::Error::custom("missing or invalid message date")),', '        }']
        elif named:
            lines += self.select_objects(name, named, '        ')
        else:
            lines += [f'        Err(D::Error::custom("invalid {name} representation"))']
        lines += ['    }', '}']
        for variant in variants:
            ty = self.rust_type(variant)
            value = 'Box::new(value)' if (name, variant.get('name')) in self.boxed else 'value'
            lines += [f'impl From<{ty}> for {name} {{',
                      f'    fn from(value: {ty}) -> Self {{ Self::{self.variant_name(variant)}({value}) }}', '}']
        return '\n'.join(lines)


def generate_types(schema):
    model = Model(schema)
    lines = ['// Generated by tools/rustgen.py. Do not edit.',
             '// Formatting is owned by the generator, not rustfmt.',
             'use serde::{Deserialize, Serialize};', 'use serde::de::Error as _;', '']
    for name, literal in sorted(model.literals.items()):
        lines += ['#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]',
                  f'pub enum {name} {{', f'    #[serde(rename = {quoted(literal)})]',
                  '    #[default]', '    Value,', '}', '']
    for name, item in sorted(model.types.items()):
        if item['kind'] == 'object':
            lines.append(model.object_source(name, item))
        elif item['kind'] == 'union':
            lines.append(model.union_source(name, item))
        elif item['kind'] == 'file':
            lines += ['/// A wire file reference (file ID, URL or attach://name), not uploaded bytes.',
                      f'pub type {name} = String;']
        else:
            raise SchemaError(f'unsupported item {name}')
        lines.append('')
    return '\n'.join(lines)


def fixture_value(schema, expr, active=frozenset()):
    """Small synthetic wire fixture; independent of the Rust model/boxing pass."""
    kind = expr['kind']
    if kind == 'primitive':
        return {'String': 'sample', 'Integer': 1, 'Float': 1.5, 'Boolean': False, 'True': True}[expr['name']]
    if kind == 'array':
        return []
    if kind == 'union':
        alternatives = expr['options']
    elif kind == 'named':
        name = expr['name']
        if name in active:
            raise SchemaError(f'fixture has a required cycle at {name}')
        active = active | {name}
        item = schema['types'][name]
        if item['kind'] == 'file':
            return 'sample'
        if item['kind'] == 'object':
            return {f['name']: f['literal'] if 'literal' in f else fixture_value(schema, f['type'], active)
                    for f in item['fields'] if f['required']}
        alternatives = item['variants']
    else:
        raise SchemaError(f'fixture expression is unsupported: {expr}')
    for alternative in alternatives:
        try:
            return fixture_value(schema, alternative, active)
        except SchemaError:
            continue
    raise SchemaError('no finite union fixture')


def generate_type_fixtures(schema):
    lines = ['// Generated by tools/rustgen.py. Do not edit.',
             'use super::types::*;',
             'fn check<T: serde::de::DeserializeOwned + serde::Serialize>(name: &str, source: &str, required: &[&str]) {',
             '    let expected: serde_json::Value = serde_json::from_str(source).unwrap();',
             '    let typed: T = serde_json::from_value(expected.clone()).unwrap_or_else(|error| panic!("{name}: {error}"));',
             '    assert_eq!(serde_json::to_value(typed).unwrap(), expected, "{name}");',
             '    for field in required {',
             '        let mut incomplete = expected.clone();',
             '        incomplete.as_object_mut().unwrap().remove(*field);',
             '        assert!(serde_json::from_value::<T>(incomplete).is_err(), "{name}.{field} must be required");',
             '    }', '}', '#[test]', 'fn every_named_type_round_trips_its_structural_fixture() {']
    for name, item in sorted(schema['types'].items()):
        value = json.dumps(fixture_value(schema, {'kind': 'named', 'name': name}), ensure_ascii=False)
        required = ', '.join(quoted(f['name']) for f in item.get('fields', []) if f['required'])
        lines.append(f'    check::<{name}>({quoted(name)}, r###"{value}"###, &[{required}]);')
    lines += ['}', '']
    return '\n'.join(lines)
