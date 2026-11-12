"""Regression checks for rating statistics, independent of chess binaries."""
import math
import unittest

from quick_elo import fit, rating_summary


class RatingTests(unittest.TestCase):
    def test_known_logistic_scores(self):
        self.assertAlmostEqual(fit([(1500, 5, 10)]), 1500)
        self.assertAlmostEqual(fit([(1500, 8, 10)]), 1500 + 400 * math.log10(4))
        self.assertAlmostEqual(fit([(1300, 8, 10), (1700, 2, 10)]), 1500)

    def test_extreme_scores_have_no_fabricated_finite_rating(self):
        self.assertEqual(fit([(1500, 10, 10)]), math.inf)
        self.assertEqual(fit([(1500, 0, 10)]), -math.inf)

    def test_only_complete_colour_pairs_count(self):
        rows = [
            {'anchor': 1500, 'opening': 0, 'candidate_white': True, 'score': 1},
            {'anchor': 1500, 'opening': 0, 'candidate_white': False, 'score': 0},
            {'anchor': 1500, 'opening': 1, 'candidate_white': True, 'score': 1},
            {'anchor': 1500, 'opening': 1, 'candidate_white': False, 'score': None},
        ]
        result = rating_summary(rows, 100)
        self.assertEqual(result['estimate'], 1500)
        self.assertEqual(result['rated_games'], 2)
        self.assertEqual(result['excluded_games'], 2)
        # Bootstrap with one opening can be degenerate; conservative interval
        # must expose the missing evidence instead of claiming exact Elo.
        self.assertEqual(result['conservative_95'], ['-inf', '+inf'])

    def test_same_opening_across_levels_is_one_cluster(self):
        rows = [{'anchor': anchor, 'opening': 0, 'candidate_white': white, 'score': .5}
                for anchor in [1300, 1700] for white in [True, False]]
        result = rating_summary(rows, 100)
        self.assertEqual(result['estimate'], 1500)
        self.assertEqual(result['opening_clusters'], 1)
        self.assertEqual(result['rated_games'], 4)


if __name__ == '__main__':
    unittest.main()
