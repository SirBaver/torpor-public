#!/usr/bin/env python3
"""Analyse OLS de la croissance des blocs orphelins du ContentStore (ADR-0055 §D4).

Lit un CSV produit par orphan-metric-sampler et évalue les deux conditions
de déclenchement du GC :
  1. Statique  : delta > max(1024, 0.02 × headers_count)
  2. Dynamique : pente OLS de delta sur fenêtre glissante 10 min > 0

Usage:
    python3 analyze.py <metrics.csv> [--window-min 10]
"""

import argparse
import csv
import sys
from dataclasses import dataclass
from typing import List


WINDOW_DEFAULT_MIN = 10


@dataclass
class Sample:
    ts_us: int
    blocks: int
    headers: int
    delta: int


def load_csv(path: str) -> List[Sample]:
    samples = []
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        for row in reader:
            samples.append(Sample(
                ts_us=int(row["timestamp_us"]),
                blocks=int(row["blocks_count"]),
                headers=int(row["headers_count"]),
                delta=int(row["delta"]),
            ))
    return samples


def ols_slope(xs: List[float], ys: List[float]) -> float:
    """Pente de la droite OLS y = a*x + b."""
    n = len(xs)
    if n < 2:
        return 0.0
    mean_x = sum(xs) / n
    mean_y = sum(ys) / n
    num = sum((x - mean_x) * (y - mean_y) for x, y in zip(xs, ys))
    den = sum((x - mean_x) ** 2 for x in xs)
    if den == 0:
        return 0.0
    return num / den


def static_threshold(headers_count: int) -> int:
    return max(1024, int(0.02 * headers_count))


def analyze(samples: List[Sample], window_min: int) -> None:
    if not samples:
        print("Aucune donnée.", file=sys.stderr)
        sys.exit(1)

    window_us = window_min * 60 * 1_000_000

    print(f"Échantillons chargés : {len(samples)}")
    print(f"Fenêtre OLS         : {window_min} min")
    print()

    latest = samples[-1]
    thresh = static_threshold(latest.headers)

    print("=== Condition statique ===")
    print(f"  delta (dernier)     : {latest.delta}")
    print(f"  seuil               : max(1024, 0.02 × {latest.headers}) = {thresh}")
    cond1 = latest.delta > thresh
    print(f"  ARMÉE               : {'OUI ⚠' if cond1 else 'NON'}")

    print()
    print("=== Condition dynamique (OLS fenêtre glissante) ===")

    t_end = samples[-1].ts_us
    t_start = t_end - window_us
    window_samples = [s for s in samples if s.ts_us >= t_start]
    n = len(window_samples)

    if n < 2:
        print(f"  Données insuffisantes dans la fenêtre ({n} échantillon(s) — minimum 2).")
        cond2 = False
    else:
        xs = [(s.ts_us - window_samples[0].ts_us) / 1e6 for s in window_samples]
        ys = [float(s.delta) for s in window_samples]
        slope = ols_slope(xs, ys)
        print(f"  Échantillons dans fenêtre : {n}")
        print(f"  Pente OLS (blocs/s)       : {slope:.4f}")
        cond2 = slope > 0
        print(f"  ARMÉE                     : {'OUI ⚠' if cond2 else 'NON'}")

    print()
    print("=== Verdict global ===")
    if cond1 and cond2:
        print("  GC DÉCLENCHÉ — les deux conditions sont satisfaites.")
        print("  → Exécuter gc_orphans en mode offline (runtime arrêté).")
    elif cond1:
        print("  Condition statique atteinte mais croissance non confirmée (pente ≤ 0).")
        print("  → Surveiller. Peut indiquer un crash ponctuel borné.")
    elif cond2:
        print("  Croissance détectée mais delta sous seuil absolu.")
        print("  → Surveiller. Pas d'action requise.")
    else:
        print("  Aucune condition armée. Pas d'action requise.")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", help="Fichier CSV produit par orphan-metric-sampler")
    parser.add_argument("--window-min", type=int, default=WINDOW_DEFAULT_MIN,
                        help=f"Durée de la fenêtre OLS en minutes (défaut : {WINDOW_DEFAULT_MIN})")
    ns = parser.parse_args()

    samples = load_csv(ns.csv)
    analyze(samples, ns.window_min)


if __name__ == "__main__":
    main()
