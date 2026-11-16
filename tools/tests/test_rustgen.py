import copy
import unittest
from tools.schema import SchemaError, import_html
from tools.tests.test_schema import FIXTURE
from tools.rustgen import Model, generate_types, generate_type_fixtures


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
