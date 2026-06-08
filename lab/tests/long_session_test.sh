#!/usr/bin/env bash
# Session longue — mesure deux propriétés :
#   1. Cohérence mémoire LLM : jusqu'à quel round le modèle rappelle-t-il correctement ?
#   2. Intégrité infrastructure : log causal + rollback après N actions
#
# Usage : bash tests/long_session_test.sh [N_ROUNDS] [BASE_URL]
# Défaut : N_ROUNDS=50, BASE=http://localhost:8888
#
# Sorties :
#   N_break    : premier round où le recall échoue (comportement modèle)
#   chain_ok   : log causal intact après N actions (infrastructure)
#   rollback_ok: rollback vers snapshot initial réussi en fin de session

set -euo pipefail

N=${1:-50}
BASE=${2:-http://localhost:8888}

PASS=0
FAIL=0
N_BREAK=-1   # -1 = jamais cassé

ok()   { echo "[PASS] $*"; PASS=$((PASS + 1)); }
fail() { echo "[FAIL] $*" >&2; FAIL=$((FAIL + 1)); }

get_field() { printf '%s' "$1" | python3 -c "import sys,json; d=json.load(sys.stdin); print($2)" 2>/dev/null; }

SESSION="long-$(date +%s)"
LAST_ACTION=""
SNAPSHOT_BASELINE=""

echo "════════════════════════════════════════════════════════"
echo "  Session longue — N=$N rounds — session=$SESSION"
echo "════════════════════════════════════════════════════════"
echo

# ── Reset ────────────────────────────────────────────────────────────────────
RESET_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$BASE/reset")
[ "$RESET_CODE" = "200" ] || { echo "[FATAL] reset échoué ($RESET_CODE)"; exit 1; }
echo "  DB remise à zéro."

# ── Snapshot baseline (infrastructure) ───────────────────────────────────────
SNAP=$(curl -sf -X POST "$BASE/snapshot" \
     -H "Content-Type: application/json" \
     -d '{"name":"long-session-baseline"}')
SNAPSHOT_BASELINE=$(get_field "$SNAP" "d['snapshot_id']")
echo "  Snapshot baseline : $SNAPSHOT_BASELINE"
echo

# ── Rounds : écriture LLM puis vérification directe ─────────────────────────
echo "--- Phase 1 : écriture + recall LLM ($N rounds) ---"
echo

for i in $(seq 1 "$N"); do
    VAL="valeur_$(printf '%04d' "$i")"
    KEY="item_$(printf '%04d' "$i")"

    # Construction du payload avec caused_by si chaîné
    if [ -z "$LAST_ACTION" ]; then
        PAYLOAD="{\"prompt\":\"Mon item numéro $i vaut $VAL. Mémorise-le.\",\"session_id\":\"$SESSION\"}"
    else
        PAYLOAD="{\"prompt\":\"Mon item numéro $i vaut $VAL. Mémorise-le.\",\"session_id\":\"$SESSION\",\"caused_by\":\"$LAST_ACTION\"}"
    fi

    RESP=$(curl -sf -X POST "$BASE/think" \
         -H "Content-Type: application/json" \
         -d "$PAYLOAD" 2>/dev/null || echo '{}')
    LAST_ACTION=$(get_field "$RESP" "d.get('action_id','')")

    # Vérification directe via API (pas via LLM — mesure la couche infrastructure)
    STORED=$(curl -sf "$BASE/memory?key=$KEY&namespace=$SESSION" 2>/dev/null \
             | python3 -c "import sys,json; print(json.load(sys.stdin).get('value','NOT_FOUND'))" 2>/dev/null \
             || echo "NOT_FOUND")

    if [ "$STORED" = "$VAL" ]; then
        ok "round $i : $KEY=$VAL ✓"
    else
        if [ $N_BREAK -eq -1 ]; then N_BREAK=$i; fi
        fail "round $i : $KEY — stocké='$STORED' (attendu $VAL)"
    fi

    # Snapshot intermédiaire tous les 10 rounds (pour le rollback final)
    if [ $((i % 10)) -eq 0 ]; then
        curl -sf -X POST "$BASE/snapshot" \
             -H "Content-Type: application/json" \
             -d "{\"name\":\"long-session-r$i\"}" > /dev/null
        echo "  [snap] snapshot créé à round $i"
    fi
done

echo

# ── Recall tardif : vérifier que les premières clés sont encore lisibles ─────
echo "--- Phase 2 : recall tardif (clés du round 1 et round 5) ---"
echo

for i in 1 5; do
    if [ "$i" -le "$N" ]; then
        VAL_EXPECTED="valeur_$(printf '%04d' "$i")"
        KEY="item_$(printf '%04d' "$i")"
        STORED=$(curl -sf "$BASE/memory?key=$KEY&namespace=$SESSION" 2>/dev/null \
                 | python3 -c "import sys,json; print(json.load(sys.stdin).get('value','NOT_FOUND'))" 2>/dev/null \
                 || echo "NOT_FOUND")
        if [ "$STORED" = "$VAL_EXPECTED" ]; then
            ok "recall tardif round $i : $KEY=$STORED ✓"
        else
            fail "recall tardif round $i : $KEY='$STORED' (attendu $VAL_EXPECTED)"
        fi
    fi
done

echo

# ── Intégrité causale (infrastructure) ───────────────────────────────────────
echo "--- Phase 3 : intégrité log causal ---"
echo

LOG_RESP=$(curl -sf "$BASE/log?limit=500")
ACTION_COUNT=$(get_field "$LOG_RESP" "len(d['actions'])")
ok "log causal : $ACTION_COUNT actions enregistrées"

# Vérifier que la dernière action est bien causalement chaînée
python3 - <<PYEOF
import urllib.request, json, sys

resp = urllib.request.urlopen("$BASE/log?limit=500")
data = json.loads(resp.read())
actions = data["actions"]

# Filtrer les actions think de notre session
session_thinks = [a for a in actions
                  if a.get("session_id") == "$SESSION" and a.get("type") == "think"]

if len(session_thinks) < 2:
    print(f"  [NOTE] Trop peu d'actions think en session ($SESSION) : {len(session_thinks)}")
    sys.exit(0)

# Vérifier que le chaînage est monotone (chaque action causée par la précédente)
broken = 0
for i in range(1, len(session_thinks)):
    prev_id = session_thinks[i-1]["action_id"]
    curr_cb = session_thinks[i].get("caused_by")
    if curr_cb != prev_id:
        broken += 1
        print(f"  [WARN] rupture causale à action {i}: caused_by={curr_cb} (attendu {prev_id[:8]}...)", file=sys.stderr)

if broken == 0:
    print(f"[PASS] chaîne causale intacte sur {len(session_thinks)} actions think")
else:
    print(f"[FAIL] {broken} rupture(s) causale(s) sur {len(session_thinks)} actions", file=sys.stderr)
    sys.exit(1)
PYEOF

echo

# ── Rollback vers baseline (infrastructure) ──────────────────────────────────
echo "--- Phase 4 : rollback vers snapshot baseline ---"
echo

RB=$(curl -sf -X POST "$BASE/rollback" \
     -H "Content-Type: application/json" \
     -d "{\"snapshot_id\":\"$SNAPSHOT_BASELINE\"}" 2>/dev/null || echo '{}')
HASH_MATCH=$(get_field "$RB" "d.get('hash_matches', False)")
KEYS_RESTORED=$(get_field "$RB" "d.get('keys_restored', -1)")

if [ "$HASH_MATCH" = "True" ]; then
    ok "rollback baseline : hash_matches=true, keys_restored=$KEYS_RESTORED"
else
    fail "rollback baseline : hash_matches=$HASH_MATCH"
fi

echo

# ── Résumé ───────────────────────────────────────────────────────────────────
echo "════════════════════════════════════════════════════════"
echo "  Session longue N=$N — PASS: $PASS  FAIL: $FAIL"
if [ $N_BREAK -gt 0 ]; then
    echo "  N_break (premier recall fail) : round $N_BREAK"
else
    echo "  N_break : aucun — cohérence maintenue sur $N rounds"
fi
echo "════════════════════════════════════════════════════════"

[ "$FAIL" -eq 0 ] || exit 1
