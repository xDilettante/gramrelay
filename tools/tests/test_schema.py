import unittest
from tools.schema import SchemaError, import_html, generate_catalog, parse_type, validate
import copy

FIXTURE = """
<div id="dev_page_content">
<p>Bot API 10.4</p>
<h4><a name="message"></a>Message</h4>
<table><thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead><tbody>
<tr><td>message_id</td><td>Integer</td><td>Identifier</td></tr>
<tr><td>text</td><td>String</td><td><em>Optional</em>. Text.</td></tr>
</tbody></table>
<h4><a name="truevariant"></a>TrueVariant</h4>
<table><thead><tr><th>Field</th><th>Type</th><th>Description</th></tr></thead><tbody>
<tr><td>type</td><td>String</td><td>Type, always “known”</td></tr>
<tr><td>is_active</td><td>True</td><td>Flag</td></tr>
</tbody></table>
<h4><a name="choice"></a>Choice</h4>
<p>This object can be one of</p><ul><li><a href="#message">Message</a></li><li><a href="#truevariant">TrueVariant</a></li></ul>
<h4><a name="sendmessage"></a>sendMessage</h4>
<p>Send text. On success, the sent <a href="#message">Message</a> is returned.</p>
<table><thead><tr><th>Parameter</th><th>Type</th><th>Required</th><th>Description</th></tr></thead><tbody>
<tr><td>chat_id</td><td>Integer or String</td><td>Yes</td><td>Target</td></tr>
<tr><td>text</td><td>String</td><td>Yes</td><td>Text</td></tr>
<tr><td>silent</td><td>Boolean</td><td>Optional</td><td>Silent</td></tr>
</tbody></table>
<h4><a name="editmessage"></a>editMessage</h4>
<p>On success, a <a href="#message">Message</a> is returned, otherwise <em>True</em> is returned.</p>
<h4><a name="empty"></a>Empty</h4><p>This object holds no information.</p>
</div>
"""

class SchemaTests(unittest.TestCase):
    def test_imports_fields_unions_methods_and_literals(self):
        schema = import_html(FIXTURE, {})
        self.assertEqual(schema["api_version"], "10.4")
        self.assertEqual(set(schema["types"]), {"Message", "TrueVariant", "Choice", "Empty"})
        fields = schema["types"]["Message"]["fields"]
        self.assertTrue(fields[0]["required"])
        self.assertFalse(fields[1]["required"])
        self.assertEqual(schema["types"]["TrueVariant"]["fields"][0]["literal"], "known")
        self.assertEqual(schema["types"]["TrueVariant"]["fields"][1]["literal"], True)
        self.assertEqual(schema["types"]["Choice"]["variants"], [
            {"kind":"named", "name":"Message"}, {"kind":"named", "name":"TrueVariant"}])
        self.assertEqual(schema["methods"]["sendMessage"]["returns"], {"kind":"named", "name":"Message"})
        self.assertEqual(schema["methods"]["editMessage"]["returns"]["kind"], "union")
        self.assertEqual(schema["types"]["Empty"], {"kind":"object","fields":[]})

    def test_type_grammar_handles_nested_arrays_and_unions(self):
        self.assertEqual(parse_type("Array of Array of String"), {
            "kind":"array","items":{"kind":"array","items":{"kind":"primitive","name":"String"}}})
        self.assertEqual(parse_type("Integer or String")["kind"], "union")
        with self.assertRaises(SchemaError):
            parse_type("a strange type?")

    def test_unquoted_emphasized_discriminator_is_preserved(self):
        html = FIXTURE.replace('Type, always “known”', 'Type, must be <em>known</em>')
        schema = import_html(html, {})
        self.assertEqual(schema["types"]["TrueVariant"]["fields"][0]["literal"], "known")

    def test_conditional_numeric_fact_is_not_an_unconditional_literal(self):
        html = FIXTURE.replace("Identifier", "Rarity. Always 0 for crafted gifts.")
        schema = import_html(html, {})
        self.assertNotIn("literal", schema["types"]["Message"]["fields"][0])
        unconditional = import_html(FIXTURE.replace("Identifier", "Always 0"), {})
        self.assertEqual(unconditional["types"]["Message"]["fields"][0]["literal"], 0)

    def test_wrapped_parameter_table_is_not_silently_discarded(self):
        start = FIXTURE.index("<table>", FIXTURE.index('name="sendmessage"'))
        end = FIXTURE.index("</table>", start) + len("</table>")
        html = FIXTURE[:start] + "<div>" + FIXTURE[start:end] + "</div>" + FIXTURE[end:]
        with self.assertRaisesRegex(SchemaError, "sendMessage"):
            import_html(html, {})

    def test_unknown_union_label_is_not_silently_deleted(self):
        html = FIXTURE.replace("</ul>", '<li><a href="#empty">Empty</a></li></ul>', 1)
        html = html.replace(">TrueVariant</a>", ">UnexpectedVariant</a>", 1)
        with self.assertRaisesRegex(SchemaError, "UnexpectedVariant"):
            import_html(html, {})
        unlinked = FIXTURE.replace("</ul>", "<li>UnexpectedVariant</li></ul>", 1)
        with self.assertRaisesRegex(SchemaError, "UnexpectedVariant"):
            import_html(unlinked, {})

    def test_unresolved_reference_is_a_hard_error(self):
        with self.assertRaisesRegex(SchemaError, "Missing"):
            import_html(FIXTURE.replace("<td>Integer</td>", "<td>Missing</td>", 1), {})

    def test_unknown_return_is_not_silently_true_or_value(self):
        broken = FIXTURE.replace("Send text. On success, the sent", "Send text. The parameter refers to").replace(
            "Message</a> is returned.</p>", "Message</a> object.</p>", 1)
        with self.assertRaisesRegex(SchemaError, "sendMessage"):
            import_html(broken, {})

    def test_unused_override_is_rejected(self):
        with self.assertRaisesRegex(SchemaError, "unused"):
            import_html(FIXTURE, {"returns":{"missingMethod":{"kind":"primitive","name":"True"}}})

    def test_redundant_field_override_is_rejected(self):
        with self.assertRaisesRegex(SchemaError, "stale"):
            import_html(FIXTURE, {"field_types":{"Message.message_id":{"kind":"primitive","name":"Integer"}}})

    def test_catalog_is_deterministic_and_contains_the_complete_inventory(self):
        schema = import_html(FIXTURE, {})
        catalog = generate_catalog(schema)
        self.assertEqual(catalog, generate_catalog(schema))
        self.assertIn('"editMessage"', catalog)
        self.assertIn('"sendMessage"', catalog)
        self.assertIn('"Choice"', catalog)
        self.assertLess(catalog.index('"editMessage"'), catalog.index('"sendMessage"'))
        self.assertIn(schema["source"]["sha256"], catalog)

    def test_schema_validation_rejects_unknown_shapes_and_invalid_fields(self):
        original = import_html(FIXTURE, {})
        changes = [
            lambda s: s.update(format_version=2),
            lambda s: s["types"]["Empty"].update(kind="unknown"),
            lambda s: s["types"]["Message"]["fields"][0].update(required="yes"),
            lambda s: s["types"]["Message"]["fields"].append(copy.deepcopy(s["types"]["Message"]["fields"][0])),
            lambda s: s["source"].update(sha256="not-a-sha256"),
        ]
        for index, change in enumerate(changes):
            schema = copy.deepcopy(original)
            change(schema)
            with self.subTest(case=index):
                with self.assertRaises(SchemaError):
                    validate(schema)

    def test_unrecognized_table_cannot_turn_into_an_empty_object(self):
        bad = FIXTURE.replace(
            "<p>This object holds no information.</p>",
            "<p>This object holds no information.</p><table><tr><th>Unexpected</th></tr><tr><td>new_field</td></tr></table>")
        with self.assertRaisesRegex(SchemaError, "Empty"):
            import_html(bad, {})

if __name__ == "__main__":
    unittest.main()
