"""Pixel continuity checks; no fonts or production data are needed."""

import unittest

from render_tui import BOX_GLYPHS, box_mask


class BorderContinuity(unittest.TestCase):
    def test_every_corner_joins_both_adjacent_strokes(self):
        for width, height in [(15, 31), (14, 28), (16, 32), (13, 27)]:
            with self.subTest(cell=(width, height)):
                masks = {symbol: box_mask(symbol, width, height) for symbol in BOX_GLYPHS}

                def row(symbol, y):
                    return [masks[symbol].getpixel((x, y)) for x in range(width)]

                def column(symbol, x):
                    return [masks[symbol].getpixel((x, y)) for y in range(height)]

                for corner in "╭╮":
                    self.assertEqual(row(corner, height - 1), row("│", 0))
                for corner in "╰╯":
                    self.assertEqual(row(corner, 0), row("│", height - 1))
                for corner in "╭╰":
                    self.assertEqual(column(corner, width - 1), column("─", 0))
                for corner in "╮╯":
                    self.assertEqual(column(corner, 0), column("─", width - 1))

                # No disconnected arc segment inside an individual cell.
                for symbol, mask in masks.items():
                    pixels = {(x, y) for y in range(height) for x in range(width)
                              if mask.getpixel((x, y)) >= 128}
                    self.assertTrue(pixels, symbol)
                    pending = [pixels.pop()]
                    while pending:
                        x, y = pending.pop()
                        # A one-pixel curved stroke can join diagonally.
                        for dx, dy in [(1, 0), (-1, 0), (0, 1), (0, -1),
                                       (1, 1), (1, -1), (-1, 1), (-1, -1)]:
                            adjacent = (x + dx, y + dy)
                            if adjacent in pixels:
                                pixels.remove(adjacent)
                                pending.append(adjacent)
                    self.assertFalse(pixels, symbol)


if __name__ == "__main__":
    unittest.main()
