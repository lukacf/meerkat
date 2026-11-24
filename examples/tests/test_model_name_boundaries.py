"""Model references in shell and Compose defaults are still example choices."""
import unittest

from model_names import NAME


class ModelNameBoundaries(unittest.TestCase):
    def test_shell_defaults_and_nested_compose_defaults(self):
        model = "gpt-" + "6-astra"
        for text in (model, f"${{MODEL:-{model}}}", f"${{A:-${{B:-{model}}}}}"):
            with self.subTest(text=text):
                self.assertEqual(NAME.findall(text), [model])

    def test_do_not_match_a_fragment_of_an_identifier(self):
        model = "gpt-" + "6-astra"
        for prefix in ("custom-", "field_", "name.", "vendor1"):
            with self.subTest(prefix=prefix):
                self.assertEqual(NAME.findall(prefix + model), [])
