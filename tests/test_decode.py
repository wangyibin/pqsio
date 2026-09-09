"""Boundary coverage for native row decoding, independent of disk I/O."""
import unittest
import pqsio as p


class DecodeTest(unittest.TestCase):
    def test_pairs_utf8_empty_null_and_integer_limits(self):
        for text in ('读段', '', None):
            native = p._Pair(text.encode() if text is not None else None,
                             2**32 - 1, 2**64 - 1, 0, 0, 43, 45, 255)
            self.assertEqual(p._decode(native, p.Pair),
                             p.Pair(text, 2**32 - 1, 2**64 - 1, 0, 0, '+', '-', 255))

    def test_concat_all_fields(self):
        for text in ('通过', '', None):
            native = p._Alignment(2**64 - 1, 100, 2, 90, 45, 3,
                                  2**32 + 1, 2**32 + 90, 255, 0.5,
                                  text.encode() if text is not None else None)
            self.assertEqual(p._decode(native, p.Alignment),
                             p.Alignment(2**64 - 1, 100, 2, 90, '-', 3,
                                         2**32 + 1, 2**32 + 90, 255, 0.5, text))

    def test_invalid_utf8_remains_an_error(self):
        with self.assertRaises(UnicodeDecodeError):
            p._decode(p._Pair(b'\xff', 0, 1, 0, 2, 43, 45, 0), p.Pair)
