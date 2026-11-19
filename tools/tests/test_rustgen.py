import copy
import hashlib
import unittest
from tools.schema import SchemaError, import_html, canonical
from tools.tests.test_schema import FIXTURE
from tools.rustgen import Model, generate_types, generate_type_fixtures, generate_methods, generate_method_fixtures, quoted


class RustGeneratorTests(unittest.TestCase):
    def test_types_are_complete_deterministic_and_strict(self):
        schema = import_html(FIXTURE, {})
        source = generate_types(schema)
        self.assertEqual(source, generate_types(schema))
        for name in schema['types']:
            self.assertIn(f'pub struct {name} ' if schema['types'][name]['kind'] == 'object'
                          else f'pub enum {name} ', source)
        self.assertIn('unknown_fields: crate::wire::UnknownFields', source)
        self.assertIn('pub is_active: crate::api::True', source)
        self.assertIn('pub enum TrueVariantType', source)
        self.assertNotIn('pub unknown_fields:', source)

    def test_size_cycles_are_cut_but_vectors_are_not_boxed(self):
        schema = import_html(FIXTURE, {})
        schema['types']['Message']['fields'].extend([
            {'name': 'parent', 'required': False, 'type': {'kind': 'named', 'name': 'Message'}},
            {'name': 'children', 'required': True, 'type': {'kind': 'array', 'items': {'kind': 'named', 'name': 'Message'}}},
        ])
        source = generate_types(schema)
        self.assertIn('pub parent: Option<Box<Message>>', source)
        self.assertIn('pub children: Vec<Message>', source)
        self.assertNotIn('Vec<Box<Message>>', source)

    def test_unknown_discriminator_layout_is_not_guessed(self):
        schema = import_html(FIXTURE, {})
        schema['types']['TrueVariant']['fields'][0]['literal'] = 1.5
        with self.assertRaises(SchemaError):
            generate_types(schema)

    def test_model_does_not_mutate_the_input(self):
        schema = import_html(FIXTURE, {})
        before = copy.deepcopy(schema)
        Model(schema)
        self.assertEqual(schema, before)

    def test_fixtures_cover_every_named_type(self):
        schema = import_html(FIXTURE, {})
        fixtures = generate_type_fixtures(schema)
        for name in schema['types']:
            self.assertEqual(fixtures.count(f'check::<{name}>('), 1)
        self.assertEqual(fixtures, generate_type_fixtures(schema))
        fixtures = generate_method_fixtures(schema)
        self.assertEqual(fixtures.count('    check::<'), len(schema['methods']))
        self.assertEqual(fixtures, generate_method_fixtures(schema))

    def test_method_policy_requires_exact_coverage_and_schema_identity(self):
        schema = import_html(FIXTURE, {})
        policy = {'schema_sha256': hashlib.sha256(canonical(schema).encode()).hexdigest(),
                  'effects': {name: 'Write' for name in schema['methods']}}
        source = generate_methods(schema, policy)
        for name in schema['methods']:
            self.assertIn(f'const NAME: &\'static str = "{name}";', source)
        self.assertEqual(source, generate_methods(schema, policy))
        policy['effects'].pop('sendMessage')
        with self.assertRaisesRegex(SchemaError, 'exactly'):
            generate_methods(schema, policy)
        policy['schema_sha256'] = '0' * 64
        with self.assertRaisesRegex(SchemaError, 'different schema'):
            generate_methods(schema, policy)

    def test_file_field_policy_is_explicit_and_rejects_stale_fields(self):
        schema = import_html(FIXTURE, {})
        policy = {'schema_sha256': hashlib.sha256(canonical(schema).encode()).hexdigest(),
                  'string_fields': ['Message.text']}
        self.assertNotIn('collect_attachments(&self.text, references)', generate_types(schema))
        self.assertIn('collect_attachments(&self.text, references)', generate_types(schema, policy))
        policy['string_fields'] = ['Message.missing']
        with self.assertRaisesRegex(SchemaError, 'stale file field'):
            generate_types(schema, policy)

    def test_rust_strings_cannot_end_a_fixture_literal(self):
        self.assertEqual(quoted('\b'), r'"\u{8}"')
        schema = import_html(FIXTURE, {})
        schema['types']['TrueVariant']['fields'][0]['literal'] = '"###; arbitrary_tokens'
        fixtures = generate_type_fixtures(schema)
        self.assertNotIn('r###"', fixtures)
        self.assertIn(r'\"###; arbitrary_tokens', fixtures)
