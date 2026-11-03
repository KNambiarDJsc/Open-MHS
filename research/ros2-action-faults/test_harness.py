"""Tests for the Layer 0 harness. Standard library only: python -m unittest -v"""

from __future__ import annotations

import math
import os
import random
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import channel  # noqa: E402
import checker  # noqa: E402
import stats  # noqa: E402
from clients import ARMS  # noqa: E402
from model import (  # noqa: E402
    RESULT_EXPIRY, SUCCEEDED, UNKNOWN, Env, Params, Plant, Server, draw_params,
)
from trial import FAULTS, SCENARIOS, run_trial, trial_seed  # noqa: E402


def params(**kw) -> Params:
    base = dict(duration=4.0, effect_frac=0.5, kill_frac=0.5, restart_delay=2.0, stall=20.0,
                crash_offset=1.0, crash_restart=2.0, meas_seed=7)
    base.update(kw)
    return Params(**base)


class StatsTests(unittest.TestCase):
    def test_clopper_pearson_zero_events_matches_closed_form(self):
        for n in (30, 100, 200, 1000):
            lo, hi = stats.clopper_pearson(0, n)
            self.assertEqual(lo, 0.0)
            self.assertAlmostEqual(hi, 1 - 0.025 ** (1 / n), places=6)

    def test_clopper_pearson_reference_value(self):
        lo, hi = stats.clopper_pearson(1, 10)
        self.assertAlmostEqual(lo, 0.002529, places=4)
        self.assertAlmostEqual(hi, 0.445007, places=4)

    def test_clopper_pearson_all_events(self):
        lo, hi = stats.clopper_pearson(20, 20)
        self.assertEqual(hi, 1.0)
        self.assertAlmostEqual(lo, 0.025 ** (1 / 20), places=6)

    def test_mcnemar_exact(self):
        self.assertEqual(stats.mcnemar_exact(0, 0), 1.0)
        self.assertAlmostEqual(stats.mcnemar_exact(0, 5), 2 * 0.5 ** 5)
        self.assertAlmostEqual(stats.mcnemar_exact(3, 3), 1.0)

    def test_holm(self):
        adj = stats.holm([0.01, 0.04, 0.03])
        self.assertAlmostEqual(adj[0], 0.03)
        self.assertAlmostEqual(adj[2], 0.06)
        self.assertAlmostEqual(adj[1], 0.06)


class ServerSemanticsTests(unittest.TestCase):
    def setUp(self):
        self.plant = Plant()
        self.p = params()
        self.server = Server(self.plant)

    def test_duplicate_uuid_rejected_while_held_then_accepted_after_expiry(self):
        self.assertEqual(self.server.accept(0.0, "u", "op", self.p), "ACCEPTED")
        self.assertEqual(self.server.accept(0.5, "u", "op", self.p), "REJECTED")
        finish = self.p.duration
        self.server.advance(finish + RESULT_EXPIRY - 0.01)
        self.assertIn("u", self.server.goals)
        self.assertEqual(self.server.goals["u"].status, SUCCEEDED)
        self.server.advance(finish + RESULT_EXPIRY + 0.01)
        self.assertNotIn("u", self.server.goals)  # an unknown goal now reads STATUS_UNKNOWN
        self.assertEqual(self.server.accept(finish + RESULT_EXPIRY + 0.02, "u", "op", self.p),
                         "ACCEPTED")  # same UUID runs again: the duplicate-effect risk

    def test_effect_applied_once_per_executed_goal(self):
        self.server.accept(0.0, "u", "op", self.p)
        self.server.advance(100.0)
        self.assertEqual(self.plant.count("op"), 1)

    def test_kill_before_effect_means_no_effect_and_clears_table(self):
        self.server.accept(0.0, "u", "op", self.p)  # effect at t=2.0
        self.server.schedule_kill(1.0, 2.0)
        self.server.advance(1.5)
        self.assertTrue(self.server.down)
        self.assertEqual(self.server.goals, {})
        self.server.advance(100.0)
        self.assertFalse(self.server.down)
        self.assertEqual(self.plant.count("op"), 0)

    def test_kill_after_effect_keeps_effect_but_loses_goal(self):
        self.server.accept(0.0, "u", "op", self.p)  # effect at 2.0, done at 4.0
        self.server.schedule_kill(3.0, 2.0)
        self.server.advance(100.0)
        self.assertEqual(self.plant.count("op"), 1)
        self.assertEqual(self.server.goals, {})

    def test_lying_driver_reports_success_without_effect(self):
        self.server.lie_next = True
        self.server.accept(0.0, "u", "op", self.p)
        self.server.advance(5.0)
        self.assertEqual(self.server.goals["u"].status, SUCCEEDED)
        self.assertEqual(self.plant.count("op"), 0)


class EnvRpcTests(unittest.TestCase):
    def test_unknown_goal_reads_unknown(self):
        env = Env(fault="F0", params=params())
        self.assertEqual(env.get_result("nope", "op"), UNKNOWN)

    def test_f1_accept_response_lost_but_goal_runs(self):
        env = Env(fault="F1", params=params())
        self.assertIsNone(env.send_goal("u", "op"))
        self.assertIn("u", env.server.goals)
        self.assertTrue(env.f1_fired)

    def test_f2_first_result_lost_then_requery_succeeds(self):
        env = Env(fault="F2", params=params())
        env.send_goal("u", "op")
        self.assertIsNone(env.get_result("u", "op"))
        self.assertEqual(env.get_result("u", "op"), SUCCEEDED)
        self.assertEqual(env.plant.count("op"), 1)

    def test_f3_server_death_during_wait_means_timeout_then_unknown(self):
        env = Env(fault="F3", params=params(kill_frac=0.9, effect_frac=0.2))
        env.send_goal("u", "op")
        self.assertIsNone(env.get_result("u", "op"))
        self.assertEqual(env.plant.count("op"), 1)  # effect happened before the kill
        self.assertEqual(env.get_result("u", "op"), UNKNOWN)  # restarted server knows nothing

    def test_f4_stall_makes_result_expire(self):
        env = Env(fault="F4", params=params(stall=30.0))
        env.send_goal("u", "op")
        self.assertEqual(env.get_result("u", "op"), UNKNOWN)

    def test_f5_success_without_effect(self):
        env = Env(fault="F5", params=params())
        env.send_goal("u", "op")
        self.assertEqual(env.get_result("u", "op"), SUCCEEDED)
        self.assertEqual(env.plant.count("op"), 0)


class ChannelTests(unittest.TestCase):
    def test_decisions(self):
        self.assertEqual(channel.decide([1, 1, 1, 1, 1]), channel.APPLIED)
        self.assertEqual(channel.decide([0, 0, 0, 0, 0]), channel.NOT_APPLIED)
        self.assertEqual(channel.decide([None] * 5), channel.UNSURE)
        self.assertEqual(channel.decide([1, None, None, None, None]), channel.UNSURE)

    def test_probabilities_sum_to_one(self):
        for applied in (True, False):
            self.assertAlmostEqual(sum(channel.decision_probabilities(applied).values()), 1.0)

    def test_analytic_decision_probabilities_match_sampling(self):
        rng = random.Random(1)
        n = 40000
        for applied in (True, False):
            p1 = (1 - channel.EPS_FN) if applied else channel.EPS_FP
            counts = {channel.APPLIED: 0, channel.NOT_APPLIED: 0, channel.UNSURE: 0}
            for _ in range(n):
                r = []
                for _ in range(channel.N_READINGS):
                    ud, uv = rng.random(), rng.random()
                    r.append(None if ud < channel.DROPOUT else (1 if uv < p1 else 0))
                counts[channel.decide(r)] += 1
            exact = channel.decision_probabilities(applied)
            for key, value in exact.items():
                se = math.sqrt(max(value * (1 - value), 1e-9) / n)
                self.assertLess(abs(counts[key] / n - value), 5 * se + 1e-3, (applied, key))


class CheckerMutationTests(unittest.TestCase):
    def history(self, final):
        return [{"event": "invoke", "op": "o"}, {"event": final, "op": "o"}]

    def test_clean_histories_pass(self):
        self.assertEqual(checker.check(self.history("ok"), [(1.0, "o")], False), [])
        self.assertEqual(checker.check(self.history("fail"), [], False), [])

    def test_duplicate_effect_flagged(self):
        v = checker.check(self.history("ok"), [(1.0, "o"), (2.0, "o")], False)
        self.assertEqual([x["type"] for x in v], ["V1"])

    def test_duplicate_effect_not_flagged_for_idempotent_control(self):
        self.assertEqual(checker.check(self.history("ok"), [(1.0, "o"), (2.0, "o")], True), [])

    def test_dropped_effect_with_ok_is_false_confirmation(self):
        v = checker.check(self.history("ok"), [], False)
        self.assertEqual([x["type"] for x in v], ["V3"])

    def test_fail_with_effect_is_false_denial(self):
        v = checker.check(self.history("fail"), [(1.0, "o")], False)
        self.assertEqual([x["type"] for x in v], ["V4"])

    def test_phantom_effect_flagged(self):
        v = checker.check(self.history("ok"), [(1.0, "o"), (1.5, "ghost")], False)
        self.assertEqual([x["type"] for x in v], ["V2"])

    def test_timeout_is_indeterminate(self):
        self.assertEqual(checker.check(self.history("timeout"), [], False), [])
        self.assertEqual(checker.check(self.history("timeout"), [(1.0, "o")], False), [])


class ClientBehaviourTests(unittest.TestCase):
    N = 40

    def run_cell(self, arm, fault, scenario="non-idempotent"):
        return [run_trial(arm, fault, scenario, trial_seed(99, fault, scenario, i))
                for i in range(self.N)]

    def test_no_fault_control_has_no_violations_for_every_arm(self):
        for arm in ARMS:
            for t in self.run_cell(arm, "F0"):
                self.assertFalse(t["V"], (arm, t))
                self.assertEqual(t["effects"], 1, (arm, t))

    def test_naive_duplicates_when_result_response_is_lost(self):
        trials = self.run_cell("A", "F2")
        self.assertTrue(all(t["D"] for t in trials))  # follows from the modelled semantics

    def test_naive_duplicate_is_harmless_for_idempotent_control(self):
        trials = self.run_cell("A", "F2", "idempotent")
        self.assertFalse(any(t["V"] for t in trials))
        self.assertTrue(all(t["effects"] >= 2 for t in trials))

    def test_careful_memory_client_survives_lost_result_and_lost_accept(self):
        for fault in ("F1", "F2"):
            for t in self.run_cell("B-M0", fault):
                self.assertFalse(t["D"], (fault, t))

    def test_memory_client_duplicates_after_client_crash_but_durable_does_not(self):
        b = [t for t in self.run_cell("B-M0", "F6") if t["fault_triggered"]]
        c = [t for t in self.run_cell("C-M0", "F6") if t["fault_triggered"]]
        self.assertTrue(b and c)
        self.assertTrue(any(t["D"] for t in b))
        self.assertFalse(any(t["D"] for t in c))

    def test_without_measurement_server_restart_leaves_unknowns(self):
        trials = [t for t in self.run_cell("C-M0", "F3") if t["fault_triggered"]]
        self.assertTrue(any(t["U"] for t in trials))
        self.assertFalse(any(t["D"] for t in trials))

    def test_false_success_is_confirmed_without_measurement_and_caught_with_it(self):
        self.assertTrue(all(t["FC"] for t in self.run_cell("A", "F5")))
        caught = sum(not t["FC"] for t in self.run_cell("C-M1", "F5"))
        self.assertGreaterEqual(caught, self.N - 2)


class DeterminismAndConsistencyTests(unittest.TestCase):
    def test_same_seed_same_result(self):
        s = trial_seed(5, "F3", "non-idempotent", 3)
        self.assertEqual(run_trial("C-M1", "F3", "non-idempotent", s),
                         run_trial("C-M1", "F3", "non-idempotent", s))

    def test_params_do_not_depend_on_arm_or_fault(self):
        s = trial_seed(5, "F3", "non-idempotent", 3)
        self.assertEqual(draw_params(s), draw_params(s))

    def test_checker_agrees_with_metric_code_on_every_cell(self):
        for fault in FAULTS:
            for scenario in SCENARIOS:
                for arm in ARMS:
                    for i in range(8):
                        t = run_trial(arm, fault, scenario, trial_seed(11, fault, scenario, i))
                        self.assertTrue(t["checker_agrees"], t)


if __name__ == "__main__":
    unittest.main(verbosity=2)
