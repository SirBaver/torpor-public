#!/usr/bin/env python3
"""FIGURE 1 (blog-04) — overhead mémoire par agent vs N, échelle log.

Génère figure-overhead-scaling.svg, SANS dépendance externe (SVG à la main).
Source des données : results/T6/SYNTHESE.md (T6-scaling, 2026-05-22, K=3 par N,
AMD Ryzen 5 PRO 4650U + WD SN530, substrat PoC Linux — R-blog-1, régime R2).

Fit retenu : overhead(N) = A + B/N, A = 9,65 KB, B = -54 KB, R² = 0,988.
Garde-fou : c'est l'overhead MÉMOIRE hébergé par agent dormant, PAS la densité
active (~70 agents/s) ni un débit. Hébergée ≠ active (cf. article §densité).

Régénérer :  python3 plot_overhead.py
"""

import math

# ── Données mesurées (T6-scaling) ───────────────────────────────────────────
POINTS = [(100, 9.1), (300, 9.5), (1000, 9.6), (3000, 9.6)]  # N, KB/agent
A, B = 9.65, -54.0          # fit overhead(N) = A + B/N
R2 = 0.988
PRED = (10000, A + B / 10000)  # prédiction 9,64 KB

# ── Géométrie ───────────────────────────────────────────────────────────────
W, H = 720, 470
ML, MR, MT, MB = 72, 28, 52, 70
PW, PH = W - ML - MR, H - MT - MB
XMIN_L, XMAX_L = math.log10(100), math.log10(10000)
YMIN, YMAX = 8.8, 9.8


def px(n):
    return ML + (math.log10(n) - XMIN_L) / (XMAX_L - XMIN_L) * PW


def py(v):
    return MT + (YMAX - v) / (YMAX - YMIN) * PH


def fr(x):  # virgule décimale française
    return f"{x:.2f}".rstrip("0").rstrip(".").replace(".", ",")


s = []
s.append(
    f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
    f'viewBox="0 0 {W} {H}" font-family="DejaVu Sans, Arial, sans-serif">'
)
s.append(f'<rect width="{W}" height="{H}" fill="#ffffff"/>')

# Titre
s.append(
    f'<text x="{W/2}" y="26" text-anchor="middle" font-size="16" '
    f'font-weight="bold" fill="#1a1a1a">Overhead mémoire par agent — scaling '
    f'(PoC Linux, régime R2)</text>'
)

# Grille Y + labels
for v in [8.8, 9.0, 9.2, 9.4, 9.6, 9.8]:
    y = py(v)
    s.append(f'<line x1="{ML}" y1="{y:.1f}" x2="{ML+PW}" y2="{y:.1f}" stroke="#e6e6e6"/>')
    s.append(
        f'<text x="{ML-10}" y="{y+4:.1f}" text-anchor="end" font-size="11" '
        f'fill="#555">{fr(v)}</text>'
    )

# Grille X + labels
for n in [100, 300, 1000, 3000, 10000]:
    x = px(n)
    s.append(f'<line x1="{x:.1f}" y1="{MT}" x2="{x:.1f}" y2="{MT+PH}" stroke="#f0f0f0"/>')
    s.append(
        f'<text x="{x:.1f}" y="{MT+PH+20}" text-anchor="middle" font-size="11" '
        f'fill="#555">{n:,}</text>'.replace(",", " ")
    )

# Cadre
s.append(
    f'<rect x="{ML}" y="{MT}" width="{PW}" height="{PH}" fill="none" stroke="#999"/>'
)

# Asymptote A = 9,65
ya = py(A)
s.append(
    f'<line x1="{ML}" y1="{ya:.1f}" x2="{ML+PW}" y2="{ya:.1f}" stroke="#c0392b" '
    f'stroke-width="1.2" stroke-dasharray="6 4"/>'
)
s.append(
    f'<text x="{ML+PW-6}" y="{ya-6:.1f}" text-anchor="end" font-size="11" '
    f'fill="#c0392b">asymptote A = {fr(A)} KB</text>'
)

# Courbe de fit overhead(N) = A + B/N
pts = []
n = 100.0
while n <= 10000:
    pts.append(f"{px(n):.1f},{py(A + B/n):.1f}")
    n *= 1.05
s.append(
    f'<polyline points="{" ".join(pts)}" fill="none" stroke="#2c6fbb" '
    f'stroke-width="2"/>'
)

# Point prédit N=10 000 (creux)
xp, yp = px(PRED[0]), py(PRED[1])
s.append(
    f'<circle cx="{xp:.1f}" cy="{yp:.1f}" r="5" fill="#ffffff" '
    f'stroke="#2c6fbb" stroke-width="2"/>'
)
s.append(
    f'<text x="{xp:.1f}" y="{yp+20:.1f}" text-anchor="middle" font-size="10" '
    f'fill="#2c6fbb">prédit {fr(PRED[1])}</text>'
)

# Points mesurés (pleins)
for n, v in POINTS:
    x, y = px(n), py(v)
    s.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="5" fill="#1a1a1a"/>')
    s.append(
        f'<text x="{x:.1f}" y="{y-10:.1f}" text-anchor="middle" font-size="10" '
        f'fill="#1a1a1a">{fr(v)}</text>'
    )

# Axes (titres)
s.append(
    f'<text x="{ML+PW/2}" y="{H-30}" text-anchor="middle" font-size="12" '
    f'fill="#333">N agents (échelle log)</text>'
)
s.append(
    f'<text x="18" y="{MT+PH/2}" text-anchor="middle" font-size="12" fill="#333" '
    f'transform="rotate(-90 18 {MT+PH/2})">overhead (KB / agent)</text>'
)

# Légende de source / garde-fou
s.append(
    f'<text x="{ML}" y="{H-10}" font-size="10" fill="#777">'
    f'Mesuré T6 (K=3) · fit overhead(N) = {fr(A)} − 54/N, R² = {fr(R2)} · '
    f'overhead hébergé (dormant), densité hébergée ≠ active</text>'
)

s.append("</svg>")

with open("figure-overhead-scaling.svg", "w", encoding="utf-8") as f:
    f.write("\n".join(s))
print("écrit : figure-overhead-scaling.svg")
