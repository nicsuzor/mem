#!/usr/bin/env python3
"""Regression tests for the three defects in epic_a463f704 Verdicts:
1. Harms edges rescaling or zeroing unrelated work, and splitting changing unrelated numbers.
2. Gain and loss netted inside one column when harming a positive target.
3. Mutual harm node ID dependence and near-saturated loops aborting whole ranking.
4. flow.py live on committed fixture printing 0 nodes.

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

FIXTURE = os.path.join(HERE, "fixtures", "live-2026-10-05.json")


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


class TestDefect3LoopHandling(unittest.TestCase):
    def test_mutual_harm_is_node_id_independent(self):
        """Defect 3: Mutual harm at full strength gives results that depend on node IDs (flow.py:121-139)."""
        g = flow.Graph()
        g.add("A")
        g.add("B")
        g.link("A", "B", 1.0, effect=flow.HARMS)
        g.link("B", "A", 1.0, effect=flow.HARMS)
        inc, _ = flow._index(g)

        res_ab = flow._settle(g, inc, {"A": 1.0, "B": 1.0}, ["A", "B"], None)
        res_ba = flow._settle(g, inc, {"A": 1.0, "B": 1.0}, ["B", "A"], None)

        self.assertAlmostEqual(res_ab["A"], res_ab["B"], places=6,
                               msg=f"Mutual harm gave asymmetric results under [A, B]: A={res_ab['A']}, B={res_ab['B']}")
        self.assertAlmostEqual(res_ab["A"], res_ba["A"], places=6,
                               msg=f"Results depend on node ID sweep order: [A, B]->{res_ab} vs [B, A]->{res_ba}")

    def test_loop_failure_does_not_abort_whole_ranking(self):
        """Defect 3: An ill-conditioned loop must not abort the ranking of independent nodes."""
        g = flow.Graph()
        g.add("CLEAN_TARGET", worth=1.0)
        g.add("CLEAN_TASK")
        g.link("CLEAN_TASK", "CLEAN_TARGET", 1.0)

        # Add a saturated loop
        g.add("L1")
        g.add("L2")
        g.link("L1", "L2", 1.0)
        g.link("L2", "L1", 1.0)

        # worth_all should rank CLEAN_TASK even if the loop component is ill-conditioned
        w = flow.worth_all(g)
        self.assertIn("CLEAN_TASK", w)
        self.assertAlmostEqual(w["CLEAN_TASK"].gain, 1.0, places=6)

    def test_engine_e3_saturated_loop_returns_null_with_named_loop(self):
        """Engine E3: Nodes feeding a saturated loop return null with the loop named."""
        g = flow.Graph()
        g.add("TARGET", worth=1.0)
        g.add("FEEDER")
        g.add("L1")
        g.add("L2")
        g.link("FEEDER", "L1", 0.5)
        g.link("L1", "L2", 1.0)
        g.link("L2", "L1", 1.0)
        g.link("L1", "TARGET", 0.8)

        w = flow.worth_all(g)
        self.assertEqual(w["FEEDER"].flow_status, "saturated_loop")
        self.assertIsNone(w["FEEDER"].gain)
        self.assertIsNone(w["FEEDER"].loss_averted)
        self.assertEqual(w["FEEDER"].loop, ["L1", "L2"])


class TestDefect6FlowLiveFixture(unittest.TestCase):
    def test_flow_live_reads_committed_fixture(self):
        """Defect 6: flow.py live on committed fixture must not print zero nodes."""
        with open(FIXTURE) as f:
            data = json.load(f)
        g = flow.from_export(data)
        self.assertGreater(len(g.state), 0, "Graph loaded 0 nodes from fixture")
        self.assertGreater(len(g.worth), 0, "Graph loaded 0 priced targets from fixture")
        self.assertGreater(len(g.edges), 0, "Graph loaded 0 edges from fixture")


if __name__ == "__main__":
    unittest.main()
