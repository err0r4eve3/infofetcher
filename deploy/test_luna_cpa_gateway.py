import json
import unittest

from luna_cpa_gateway import extract_usage


class ExtractUsageTest(unittest.TestCase):
    def test_chat_completion_usage(self) -> None:
        body = json.dumps(
            {
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 20,
                    "total_tokens": 120,
                    "prompt_tokens_details": {"cached_tokens": 80},
                    "completion_tokens_details": {"reasoning_tokens": 5},
                }
            }
        ).encode()

        usage = extract_usage(body)

        self.assertEqual(usage["input_tokens"], 100)
        self.assertEqual(usage["cached_input_tokens"], 80)
        self.assertEqual(usage["output_tokens"], 20)
        self.assertEqual(usage["reasoning_tokens"], 5)
        self.assertEqual(usage["total_tokens"], 120)

    def test_invalid_response_has_no_usage(self) -> None:
        self.assertEqual(extract_usage(b"not-json"), {})


if __name__ == "__main__":
    unittest.main()
