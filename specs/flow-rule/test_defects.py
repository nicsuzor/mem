#!/usr/bin/env python3
"""Regression tests for the three defects in epic_a463f704 Verdicts:
1. Harms edges rescaling or zeroing unrelated work, and splitting changing unrelated numbers.
2. Gain and loss netted inside one column when harming a positive target.

Each test fails at head 15e43e9 and passes with the fixes.
"""

import copy
import json
import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import flow


class TestDefect1HarmsGuarantees(unittest.TestCase):
    def test_open_harm_does_not_rescale_or_zero_unrelated_work(self):
        """Defect 1: An open harmful node must not rescale or zero unrelated work Z."""
        g = flow.Graph()
        g.add("T", worth=1.0)
        g.add("Z")
        g.link("Z", "T", 1.0)  # Z helps T at 1.0

        # Without H
        w_clean = flow.worth_all(g)
        self.assertAlmostEqual(w_clean["Z"].gain, 1.0, places=6)

        # Add open harmful node H harming T at 0.5
        g.add("H")
        g.link("H", "T", 0.5, effect=flow.HARMS)
        w_half = flow.worth_all(g)
        self.assertAlmostEqual(w_half["Z"].gain, 1.0, places=6,
                               msg=f"Z gain was deflated to {w_half['Z'].gain} by open harmful node H")

        # Increase H harm to 1.0
        for e in g.edges:
            if e.src == "H":
                e.quantum = 1.0
        w_full = flow.worth_all(g)
        self.assertAlmostEqual(w_full["Z"].gain, 1.0, places=6,
                               msg=f"Z gain was zeroed to {w_full['Z'].gain} by open harmful node H at strength 1.0")

    def test_splitting_harmful_node_preserves_unrelated_numbers(self):
        """Defect 1: Splitting an open harmful node must not change unrelated node's numbers (Invariant 2, P4)."""
        g1 = flow.Graph()
        g1.add("T", worth=1.0)
        g1.add("Z")
        g1.link("Z", "T", 1.0)
        g1.add("H")
        g1.link("H", "T", 0.6, effect=flow.HARMS)
        w1 = flow.worth_all(g1)

        g2 = flow.Graph()
        g2.add("T", worth=1.0)
        g2.add("Z")
        g2.link("Z", "T", 1.0)
        g2.add("H1")
        g2.add("H2")
        g2.link("H1", "T", 0.3, effect=flow.HARMS)
        g2.link("H2", "T", 0.3, effect=flow.HARMS)
        w2 = flow.worth_all(g2)

        self.assertAlmostEqual(w1["Z"].gain, w2["Z"].gain, places=6,
                               msg=f"Splitting H changed Z gain from {w1['Z'].gain} to {w2['Z'].gain}")


class TestDefect2GainLossSeparation(unittest.TestCase):
    def test_gain_and_loss_not_netted_inside_gain_column(self):
        """Defect 2: Work that serves one positive target at 0.6 and harms another at 0.6

        must not read (0, 0) like unlinked work. Settled point 16, invariant 15.
        """
        g = flow.Graph()
        g.add("T1", worth=1.0)
        g.add("T2", worth=1.0)
        g.add("U")
        g.link("U", "T1", 0.6)  # helps T1
        g.link("U", "T2", 0.6, effect=flow.HARMS)  # harms T2

        g.add("UNLINKED")

        w = flow.worth_all(g)
        self.assertNotEqual((w["U"].gain, w["U"].loss_averted), (0.0, 0.0),
                            msg="Work with equal positive gain and harm to positive target was netted to (0, 0)")
        self.assertAlmostEqual(w["U"].gain, 0.6, places=6)
        self.assertAlmostEqual(w["U"].loss_averted, -0.6, places=6)


if __name__ == "__main__":
    unittest.main()
