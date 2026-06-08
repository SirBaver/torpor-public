//! demo_tui — démonstrateur TUI live de l'OS-pour-IA. Sélecteur de scène `--scene`.
//!
//! ## Scènes
//!
//! `--scene effects` (défaut) — **contrôle des effets (R1)** : pipeline `reviewer` (WASM)
//!   → `judge` (WASM), DAG causal en temps réel, + trois temps forts au clavier :
//!   [t] FALSIFICATION  : altère 1 octet d'une entrée, recalcule action_id,
//!                        montre stored ≠ recalculé + le juge orphelin (P3 — intégrité, lecture seule).
//!   [r] ROLLBACK       : Message::Rollback sur l'agent mémoire vivant → annulation
//!                        atomique d'état, entrée SchedulerRollback dans le log (P2).
//!   [x] INTRUS         : agent capability-gated tente un accès hors scope → bloqué à la
//!                        frontière, CapabilityDenied (0x14) tracé par le runtime (P4).
//!
//! `--scene mission-resume` — **un agent accomplit une mission** (tâche 4 étapes) et la
//!   reprend après une interruption *simulée* : la RAM WASM est effacée, le contexte est
//!   relu depuis le log causal, les étapes déjà faites ne re-déclenchent pas le LLM.
//!   Propriété : **P3 (traçabilité) — le log est la source de vérité des résultats émis**.
//!   PAS P1a (aucune RAM mesurée), PAS P6 (interruption simulée, pas un crash). Régime R1.
//!
//! `--scene incident` — **triage multi-agent** : fan-out 3 spécialistes (infra/db/sécurité)
//!   → fan-in agrégateur. Le DAG causal cross-agent est le héros visuel. P3 B-light
//!   mono-tenant (ADR-0036), régime R1 — même statut que la scène effects.
//!
//! `--scene swarm` — **mécanisme d'ordonnancement** (PAS une mesure de densité) :
//!   Acte 1 admission bornée (C2, IoAdmissionQueue, in-flight ≤ cap garanti sémaphore) ;
//!   Acte 2 densité par éviction → dormant (Scheduler) / réveil depuis snapshot (S11/S12).
//!   Compteurs RÉELS uniquement. Backend simulé (SleepyBackend). Aucune densité revendiquée,
//!   aucun ~100 agents/s : N à l'écran ≠ N soutenables (garde-fou architecte / spec/07).
//!
//! ## Périmètre & honnêteté
//!
//! Périmètre `use case` : aucun fichier du cœur runtime modifié. Backends (`CannedBackend`,
//! `SeqBackend`) locaux au binaire ; la TUI est en lecture seule sur le log + pilotage des
//! primitives existantes (Message::data/caused/Rollback, grant_root, run_loop).
//!
//! Garde-fou F1 : le rejeu prouve le contrôle des EFFETS, pas une performance d'inférence.
//! L'écran affiche `mode: rejeu/LIVE` et le régime en permanence. Substrat Linux : les
//! verdicts ne transfèrent pas à seL4 (D7).
//!
//! Build  : depuis `poc/`, avec les agents WASM compilés :
//!   cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
//!     --example code_reviewer --example severity_judge --example multi_turn \
//!     --example data_accessor --example task_step --example incident_aggregator
//!   cargo run -p os-poc-runtime --features demo-tui --bin demo-tui -- [--scene <nom>] [--live]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use os_poc_capabilities::{CapabilityStore, Permissions};
use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType, LogEntry};
use os_poc_runtime::actor::{
    ActorInstance, AgentId, Message, SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
};
use os_poc_runtime::inference::{
    InferError, InferResponse, InferenceBackend, InferencePool, OllamaBackend, PriorityClass,
};
use os_poc_runtime::actor::AGENT_WAT;
use os_poc_runtime::io_queue::IoAdmissionQueue;
use os_poc_runtime::scheduler::Scheduler;
use os_poc_runtime::watchdog::AgentProfile;
use os_poc_runtime::{load_module_from_file, make_engine};
use os_poc_store::{Cache, ContentStore};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

// ── Données de la démo (mode rejeu) ────────────────────────────────────────────

const CODE_SNIPPET: &str = "def login(db, user, pwd):\n    sql = \"SELECT * FROM users WHERE name='\" + user + \"'\"\n    row = db.execute(sql).fetchone()\n    return row if row and row['password'] == pwd else None";

const REVIEW_CANNED: &str = "[BLOCKER] login: injection SQL via concaténation de chaîne\n[BLOCKER] login: comparaison de mot de passe en clair\n[WARNING] login: aucune limite de tentatives";

const VERDICT_CANNED: &str = "VERDICT: REJECT — 2 BLOCKER à corriger avant merge";

// Variante « LLM faillible » (--llm-wrong) : le reviewer RATE la faille, le juge approuve.
// Démontre honnêtement que le système contrôle les EFFETS, pas la justesse sémantique du LLM
// (frontière LLM = non-objectif). Le mauvais verdict est tout de même tracé et attribué.
const REVIEW_WRONG: &str = "RAS — le code paraît correct, rien à signaler.";
const VERDICT_WRONG: &str = "VERDICT: APPROVE — aucun problème détecté";

const MEMO_CANNED: &str = "noté.";

// Identités (16 octets chacune).
const ID_REVIEWER: AgentId = *b"reviewer-agent00";
const ID_JUDGE: AgentId = *b"judge-agent00000";
const ID_MEMO: AgentId = *b"memo-agent000000";
const ID_ROGUE: AgentId = *b"rogue-agent00000";

// ── Backend rejeu : réponse en conserve keyée par agent_id ─────────────────────

#[derive(Clone)]
struct CannedBackend {
    responses: Arc<HashMap<AgentId, String>>,
    delay_ms: u64,
}

impl InferenceBackend for CannedBackend {
    async fn infer(
        &self,
        agent_id: &AgentId,
        _prompt: &[u8],
        _timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        let text = self
            .responses
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| "[rejeu: pas de réponse en conserve]".to_string());
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(InferError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {
                Ok(InferResponse { text, truncated: false, slot_info: None })
            }
        }
    }
}

/// Backend rejeu *séquentiel* : pour chaque agent, une file de réponses consommées
/// dans l'ordre (une par appel d'inférence). Permet à un même `agent_id` — l'agent de
/// mission qui exécute 4 étapes successives — de renvoyer une réponse différente par
/// étape, ce que `CannedBackend` (keyé agent_id, réponse unique) ne sait pas faire.
#[derive(Clone)]
struct SeqBackend {
    responses: Arc<Mutex<HashMap<AgentId, std::collections::VecDeque<String>>>>,
    delay_ms: u64,
}

impl InferenceBackend for SeqBackend {
    async fn infer(
        &self,
        agent_id: &AgentId,
        _prompt: &[u8],
        _timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        // Le guard Mutex est relâché à la fin de cette ligne (avant le select await).
        let text = self
            .responses
            .lock()
            .unwrap()
            .get_mut(agent_id)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| "[rejeu : file de réponses épuisée]".to_string());
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(InferError::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {
                Ok(InferResponse { text, truncated: false, slot_info: None })
            }
        }
    }
}

/// Backend de la démo : rejeu en conserve (keyé agent), rejeu séquentiel (file par
/// agent), ou live (Ollama réel via --live).
enum DemoBackend {
    Canned(CannedBackend),
    Seq(SeqBackend),
    Ollama(OllamaBackend),
}

impl InferenceBackend for DemoBackend {
    async fn infer(
        &self,
        agent_id: &AgentId,
        prompt: &[u8],
        timeout_ms: u32,
        cancel: CancellationToken,
    ) -> Result<InferResponse, InferError> {
        match self {
            DemoBackend::Canned(b) => b.infer(agent_id, prompt, timeout_ms, cancel).await,
            DemoBackend::Seq(b) => b.infer(agent_id, prompt, timeout_ms, cancel).await,
            DemoBackend::Ollama(b) => b.infer(agent_id, prompt, timeout_ms, cancel).await,
        }
    }
}

// ── Projection lecture seule du log ────────────────────────────────────────────

struct Node {
    agent: String,
    hash: [u8; 32],
    etype: String,
    parents: Vec<[u8; 32]>,
}

/// Rapport de falsification : recalcul d'action_id après mutation d'un octet.
struct TamperReport {
    target: [u8; 32],     // clé stockée (= action_id original) référencée par les enfants
    recomputed: [u8; 32], // action_id recalculé après mutation → diverge
}

fn hex8(b: &[u8; 32]) -> String {
    b.iter().take(4).map(|x| format!("{x:02x}")).collect()
}

fn full_hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn env_of(e: &LogEntry) -> Option<EmitEnvelope> {
    e.emit_payload
        .as_ref()
        .and_then(|pb| EmitEnvelope::from_msgpack(pb).ok())
}

fn find_entry_by_type(log: &CausalLog, id: &AgentId, etype: u8) -> Option<([u8; 32], LogEntry)> {
    for aid in log.query_by_agent_range(id, None, None).unwrap_or_default() {
        if let Ok(Some(e)) = log.get(&aid) {
            if env_of(&e).map(|env| env.emit_type == etype).unwrap_or(false) {
                return Some((aid, e));
            }
        }
    }
    None
}

fn collect_dag(log: &CausalLog, ids: &[(AgentId, &str)]) -> Vec<Node> {
    let mut out = Vec::new();
    for (id, name) in ids {
        let aids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        for aid in aids {
            if let Ok(Some(e)) = log.get(&aid) {
                let etype = etype_label(&e);
                out.push(Node {
                    agent: name.to_string(),
                    hash: aid,
                    etype,
                    parents: e.parent_ids.iter().copied().collect(),
                });
            }
        }
    }
    out
}

fn etype_label(e: &LogEntry) -> String {
    match &e.emit_payload {
        Some(pb) => EmitEnvelope::from_msgpack(pb)
            .map(|env| match env.emit_type {
                t if t == EmitType::ActionResult as u8 => "ActionResult".to_string(),
                t if t == EmitType::Event as u8 => "Event".to_string(),
                t if t == EmitType::SchedulerRollback as u8 => "SchedulerRollback".to_string(),
                t if t == EmitType::SelfRollback as u8 => "SelfRollback".to_string(),
                t if t == EmitType::CapabilityDenied as u8 => "CapabilityDenied".to_string(),
                other => format!("emit 0x{other:02x}"),
            })
            .unwrap_or_else(|_| "?".into()),
        None => "Commit".into(),
    }
}

fn latest_action_result(log: &CausalLog, id: &AgentId) -> Option<(String, [u8; 32])> {
    let aids = log.query_by_agent_range(id, None, None).unwrap_or_default();
    for aid in aids.iter().rev() {
        if let Ok(Some(e)) = log.get(aid) {
            if let Some(pb) = &e.emit_payload {
                if let Ok(env) = EmitEnvelope::from_msgpack(pb) {
                    if env.emit_type == EmitType::ActionResult as u8 {
                        let txt = String::from_utf8_lossy(&env.payload).trim().to_string();
                        return Some((txt, *aid));
                    }
                }
            }
        }
    }
    None
}

fn count_action_results(log: &CausalLog, id: &AgentId) -> usize {
    log.query_by_agent_range(id, None, None)
        .unwrap_or_default()
        .iter()
        .filter(|aid| {
            log.get(aid)
                .ok()
                .flatten()
                .and_then(|e| e.emit_payload)
                .and_then(|pb| EmitEnvelope::from_msgpack(&pb).ok())
                .map(|env| env.emit_type == EmitType::ActionResult as u8)
                .unwrap_or(false)
        })
        .count()
}

fn has_emit_type(log: &CausalLog, id: &AgentId, etype: u8) -> bool {
    log.query_by_agent_range(id, None, None)
        .unwrap_or_default()
        .iter()
        .any(|aid| {
            log.get(aid)
                .ok()
                .flatten()
                .and_then(|e| e.emit_payload)
                .and_then(|pb| EmitEnvelope::from_msgpack(&pb).ok())
                .map(|env| env.emit_type == etype)
                .unwrap_or(false)
        })
}

/// Construit le rapport de falsification : récupère l'entrée réelle, mute un octet
/// d'une copie, recalcule l'action_id. Démonstration tamper-evident, sans écrire.
fn make_tamper(log: &CausalLog, target: &[u8; 32]) -> Option<TamperReport> {
    let entry = log.get(target).ok()??;
    let mut t = entry.clone();
    match &mut t.emit_payload {
        Some(p) if !p.is_empty() => p[0] ^= 0xFF,
        _ => t.ts_ms ^= 0x1,
    }
    let recomputed = t.action_id();
    Some(TamperReport { target: *target, recomputed })
}

// ── État de scénario ───────────────────────────────────────────────────────────

#[derive(PartialEq, Clone, Copy)]
enum Step {
    Start,
    ReviewerRunning,
    ReviewerDone,
    JudgeRunning,
    Done,
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Pipeline,
    Tamper,
    Rollback,
    Intrus,
}

/// Faits dérivés du log à chaque tick (passés au rendu).
struct Facts {
    dag: Vec<Node>,
    memo_actions: usize,
    memo_rolled: bool,
    rogue_last: Option<String>,
    rogue_denied: bool,
}

// ── Boucle UI (synchrone, exécutée via spawn_blocking) ─────────────────────────

struct Senders {
    reviewer: Sender<Message>,
    judge: Sender<Message>,
    memo: Sender<Message>,
    rogue: Sender<Message>,
}

fn run_ui(log: Arc<CausalLog>, tx: Senders, cap_id: u64, live: bool, code: String, llm_wrong: bool) {
    let mut terminal = ratatui::init();
    let ids = [(ID_REVIEWER, "reviewer"), (ID_JUDGE, "judge")];
    let mut step = Step::Start;
    let mut focus = Focus::Pipeline;
    let mut review: Option<(String, [u8; 32])> = None;
    let mut verdict: Option<(String, [u8; 32])> = None;
    let mut tamper: Option<TamperReport> = None;
    let mut drill = false;

    loop {
        // Avancement automatique du pipeline selon l'état réel du log.
        match step {
            Step::ReviewerRunning => {
                if let Some(r) = latest_action_result(&log, &ID_REVIEWER) {
                    review = Some(r);
                    step = Step::ReviewerDone;
                }
            }
            Step::JudgeRunning => {
                if let Some(v) = latest_action_result(&log, &ID_JUDGE) {
                    verdict = Some(v);
                    step = Step::Done;
                }
            }
            _ => {}
        }

        let facts = Facts {
            dag: collect_dag(&log, &ids),
            memo_actions: count_action_results(&log, &ID_MEMO),
            memo_rolled: has_emit_type(&log, &ID_MEMO, EmitType::SchedulerRollback as u8)
                || has_emit_type(&log, &ID_MEMO, EmitType::SelfRollback as u8),
            rogue_last: latest_action_result(&log, &ID_ROGUE).map(|(t, _)| t),
            rogue_denied: has_emit_type(&log, &ID_ROGUE, EmitType::CapabilityDenied as u8),
        };

        if terminal
            .draw(|f| draw(f, step, focus, drill, live, llm_wrong, &log, &facts, &review, &verdict, &tamper))
            .is_err()
        {
            break;
        }

        match event::poll(Duration::from_millis(120)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char(' ') => match step {
                                Step::Start => {
                                    let _ = tx
                                        .reviewer
                                        .blocking_send(Message::data(code.as_bytes().to_vec()));
                                    step = Step::ReviewerRunning;
                                    focus = Focus::Pipeline;
                                }
                                Step::ReviewerDone => {
                                    if let Some((txt, aid)) = &review {
                                        let _ = tx.judge.blocking_send(Message::caused(
                                            txt.as_bytes().to_vec(),
                                            *aid,
                                        ));
                                        step = Step::JudgeRunning;
                                        focus = Focus::Pipeline;
                                    }
                                }
                                _ => {}
                            },
                            // [t] FALSIFICATION — toggle, lecture seule sur le rapport du reviewer.
                            KeyCode::Char('t') => {
                                if tamper.is_some() {
                                    tamper = None;
                                    focus = Focus::Pipeline;
                                } else if let Some((_, aid)) = &review {
                                    tamper = make_tamper(&log, aid);
                                    focus = Focus::Tamper;
                                }
                            }
                            // [r] ROLLBACK — annulation atomique de l'agent mémoire.
                            KeyCode::Char('r') => {
                                let _ = tx.memo.blocking_send(Message::Rollback { target_seq: 1 });
                                focus = Focus::Rollback;
                            }
                            // [x] INTRUS — accès hors scope → refusé à la frontière.
                            KeyCode::Char('x') => {
                                let mut msg = cap_id.to_le_bytes().to_vec();
                                msg.extend_from_slice(b"confidential/salaires_2024");
                                let _ = tx.rogue.blocking_send(Message::data(msg));
                                focus = Focus::Intrus;
                            }
                            // [d] PREUVE — déplie/replie le panneau de détails vérifiables.
                            KeyCode::Char('d') => drill = !drill,
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    }

    ratatui::restore();
}

/// Panneau PREUVE ([d]) — détails vérifiables du moment en focus, lus du vrai log.
fn drill_lines(
    log: &CausalLog,
    focus: Focus,
    review: &Option<(String, [u8; 32])>,
    verdict: &Option<(String, [u8; 32])>,
    tamper: &Option<TamperReport>,
) -> Vec<Line<'static>> {
    let title = |s: &str| {
        Line::from(Span::styled(
            s.to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
    };
    let label = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(Color::Gray)));
    let kv = |k: &str, v: &str| {
        Line::from(vec![
            Span::styled(format!("{k} : "), Style::default().fg(Color::DarkGray)),
            Span::styled(v.to_string(), Style::default().fg(Color::White)),
        ])
    };
    let good = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(Color::Green)));
    // Hash complet sur sa propre ligne indentée (64 hex tiennent dans le panneau).
    let hashln = |k: &str, v: String, col: Color| {
        vec![
            Line::from(Span::styled(format!("{k} :"), Style::default().fg(Color::DarkGray))),
            Line::from(Span::styled(format!("  {v}"), Style::default().fg(col))),
        ]
    };

    let mut out: Vec<Line<'static>> = Vec::new();
    match focus {
        Focus::Tamper => {
            out.push(title("PREUVE — tamper-evidence (P3 — intégrité)"));
            match tamper {
                Some(t) => {
                    out.push(label("entrée ciblée : rapport du reviewer (ActionResult)"));
                    out.extend(hashln("action_id stocké", full_hex(&t.target), Color::White));
                    out.extend(hashln("action_id recalculé", full_hex(&t.recomputed), Color::Red));
                    out.push(good("→ divergent dès l'octet muté : falsification visible."));
                    out.push(label(""));
                    out.push(label("action_id = SHA256(bincode(LogEntry))"));
                    out.push(label("réf : poc/causal-log/src/lib.rs:188"));
                    out.push(label("→ tout enfant pointant l'id stocké devient orphelin."));
                }
                None => out.push(label("Appuie sur [t] pour falsifier, puis [d].")),
            }
        }
        Focus::Rollback => {
            out.push(title("PREUVE — rollback atomique (P2)"));
            let entry = find_entry_by_type(log, &ID_MEMO, EmitType::SchedulerRollback as u8)
                .or_else(|| find_entry_by_type(log, &ID_MEMO, EmitType::SelfRollback as u8));
            match entry {
                Some((aid, e)) => {
                    out.extend(hashln("entrée rollback", full_hex(&aid), Color::White));
                    out.push(kv("agent", "memo-agent000000"));
                    out.push(kv("seq cible", "1"));
                    if let Some(env) = env_of(&e) {
                        out.push(kv("emit_type", &format!("0x{:02x}", env.emit_type)));
                    }
                    out.push(good("→ état restauré ; aucune entrée intermédiaire observable."));
                }
                None => out.push(label("Appuie sur [r] pour annuler l'état mémoire, puis [d].")),
            }
        }
        Focus::Intrus => {
            out.push(title("PREUVE — refus de capability (P4)"));
            match find_entry_by_type(log, &ID_ROGUE, EmitType::CapabilityDenied as u8) {
                Some((aid, e)) => {
                    out.extend(hashln("entrée CapabilityDenied", full_hex(&aid), Color::White));
                    out.push(kv("agent", "rogue-agent00000"));
                    if let Some(env) = env_of(&e) {
                        let shown: String = String::from_utf8_lossy(&env.payload)
                            .chars()
                            .filter(|c| c.is_ascii_graphic() || *c == ' ')
                            .collect();
                        out.push(kv("payload (ressource)", shown.trim()));
                    }
                    out.push(good("→ refus émis PAR LE RUNTIME, pas par l'agent."));
                }
                None => out.push(label("Appuie sur [x] pour la tentative d'intrusion, puis [d].")),
            }
        }
        Focus::Pipeline => {
            out.push(title("PREUVE — lien causal cross-agent (P3 — intégrité, B-light)"));
            match (verdict, review) {
                (Some((vtext, vaid)), Some((rtext, raid))) => {
                    if let Ok(Some(e)) = log.get(vaid) {
                        out.extend(hashln("verdict action_id", full_hex(vaid), Color::White));
                        let matched = e.parent_ids.iter().find(|p| *p == raid);
                        match matched {
                            Some(p) => {
                                out.extend(hashln("parent (cause)", full_hex(p), Color::Green));
                                out.push(good("→ ce parent EST l'action_id du rapport, octet pour octet."));
                            }
                            None => {
                                if let Some(p) = e.parent_ids.first() {
                                    out.extend(hashln("parent (cause)", full_hex(p), Color::White));
                                }
                            }
                        }
                        if let Some(env) = env_of(&e) {
                            out.push(label(""));
                            out.push(label("payload (EmitEnvelope décodé) :"));
                            out.push(kv(
                                "  emit_type",
                                &format!("0x{:02x} (ActionResult)", env.emit_type),
                            ));
                            out.push(kv(
                                "  agent_id",
                                String::from_utf8_lossy(&env.agent_id).trim_end_matches('\0'),
                            ));
                            out.push(kv("  seq", &env.seq.to_string()));
                        }
                        // Causes lisibles du verdict : rapport complet du reviewer + verdict
                        // du juge (peuvent être longs en mode --live → wrap actif sur le panneau).
                        out.push(label(""));
                        out.push(title("RAPPORT du reviewer (les causes) :"));
                        for l in rtext.lines() {
                            out.push(label(&format!("  {}", l.trim_end())));
                        }
                        out.push(label(""));
                        out.push(title("VERDICT du juge :"));
                        for l in vtext.lines() {
                            out.push(label(&format!("  {}", l.trim_end())));
                        }
                    }
                }
                _ => out.push(label("Déroule la revue ([espace] ×2) puis [d] pour la preuve.")),
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw(
    f: &mut Frame,
    step: Step,
    focus: Focus,
    drill: bool,
    live: bool,
    llm_wrong: bool,
    log: &CausalLog,
    facts: &Facts,
    review: &Option<(String, [u8; 32])>,
    verdict: &Option<(String, [u8; 32])>,
    tamper: &Option<TamperReport>,
) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(4),
        ])
        .split(f.area());

    // En-tête + bannière régime/mode (honnêteté à l'écran).
    let header = Line::from(vec![
        Span::styled(
            " OS-pour-IA · DÉMONSTRATEUR LIVE ",
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        if live {
            Span::styled(
                "mode: LIVE (Ollama)",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("mode: rejeu", Style::default().fg(Color::Magenta))
        },
        Span::raw(" · "),
        Span::styled("régime: R1 (effets)", Style::default().fg(Color::Green)),
        Span::raw(" · "),
        Span::styled("substrat: Linux", Style::default().fg(Color::DarkGray)),
        if llm_wrong && !live {
            Span::styled(
                "  · scénario: LLM faillible",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("")
        },
    ]);
    f.render_widget(Paragraph::new(header), root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(root[1]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(0)])
        .split(body[0]);

    // Panneau AGENTS (4 agents).
    let (state_r, color_r) = match step {
        Step::Start => ("○ prêt", Color::DarkGray),
        Step::ReviewerRunning => ("● infère… (rejeu)", Color::Cyan),
        _ => ("✓ rapport émis", Color::Blue),
    };
    let (state_j, color_j) = match step {
        Step::JudgeRunning => ("● infère… (rejeu)", Color::Cyan),
        Step::Done => ("✓ verdict émis", Color::Yellow),
        _ => ("○ en attente", Color::DarkGray),
    };
    let memo_state = if facts.memo_rolled {
        "↶ rollback → seq 1 (P2)".to_string()
    } else {
        format!("● vivant · {} tours", facts.memo_actions)
    };
    let memo_color = if facts.memo_rolled { Color::Green } else { Color::DarkGray };
    let rogue_state = if facts.rogue_denied {
        "⛔ accès REFUSÉ (0x14)".to_string()
    } else {
        "○ dans son périmètre".to_string()
    };
    let rogue_color = if facts.rogue_denied { Color::Red } else { Color::DarkGray };
    let agents = vec![
        agent_line("reviewer", Color::Blue, state_r, color_r),
        agent_line("judge", Color::Yellow, state_j, color_j),
        agent_line("memo", Color::Magenta, &memo_state, memo_color),
        agent_line("intrus", Color::Red, &rogue_state, rogue_color),
    ];
    f.render_widget(
        Paragraph::new(agents).block(Block::default().borders(Borders::ALL).title(" AGENTS ")),
        left[0],
    );

    // Panneau PROPRIÉTÉS.
    let on = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);
    let off = Style::default().fg(Color::DarkGray);
    let alert = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK);
    let p2 = if facts.memo_rolled {
        Line::from(Span::styled("P2 rollback      ▣ ACTIVE", on))
    } else {
        Line::from(Span::styled("P2 rollback      ▢", off))
    };
    let p3 = if tamper.is_some() {
        Line::from(Span::styled("P3 traçabilité   ▣ VIOLATION DÉTECTÉE", alert))
    } else {
        Line::from(Span::styled("P3 traçabilité   ▣ ACTIVE", on))
    };
    let p4 = if facts.rogue_denied {
        Line::from(Span::styled("P4 isolation     ▣ ACTIVE", on))
    } else {
        Line::from(Span::styled("P4 isolation     ▢", off))
    };
    let props = vec![
        p2,
        p3,
        p4,
        Line::from(Span::styled("P6 atomicité     ▣ ACTIVE", on)),
        Line::from(""),
        Line::from(Span::styled(
            "[t] falsifier  [r] rollback  [x] intrus",
            Style::default().fg(Color::Gray),
        )),
    ];
    f.render_widget(
        Paragraph::new(props).block(Block::default().borders(Borders::ALL).title(" PROPRIÉTÉS ")),
        left[1],
    );

    // Panneau DAG — construit depuis le vrai log, annoté si falsification.
    let mut dag_lines: Vec<Line> = vec![Line::from(Span::styled(
        "● code source (genesis)",
        Style::default().fg(Color::DarkGray),
    ))];
    for n in &facts.dag {
        let is_tampered = tamper.as_ref().map(|t| t.target == n.hash).unwrap_or(false);
        let is_orphan = tamper
            .as_ref()
            .map(|t| n.parents.iter().any(|p| *p == t.target))
            .unwrap_or(false);
        let agent_color = if n.agent == "reviewer" { Color::Blue } else { Color::Yellow };

        if is_tampered {
            let t = tamper.as_ref().unwrap();
            dag_lines.push(Line::from(vec![
                Span::styled("●── ", Style::default().fg(Color::Red)),
                Span::styled(
                    format!("{:<9} ", n.agent),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("stocké {} ≠ recalc {} ", hex8(&t.target), hex8(&t.recomputed)),
                    Style::default().fg(Color::Red),
                ),
                Span::styled("⚠ FALSIFIÉ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            ]));
            continue;
        }

        if is_orphan {
            // Annotation compacte (placée tôt sur la ligne pour rester visible).
            dag_lines.push(Line::from(vec![
                Span::styled("●── ", Style::default().fg(Color::Red)),
                Span::styled(
                    format!("{:<9} ", n.agent),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(hex8(&n.hash), Style::default().fg(Color::White)),
                Span::styled(
                    "  ⚠ ORPHELIN (parent falsifié)",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]));
            continue;
        }

        let mut spans = vec![
            Span::styled("●── ", Style::default().fg(agent_color)),
            Span::styled(
                format!("{:<9} ", n.agent),
                Style::default().fg(agent_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(hex8(&n.hash), Style::default().fg(Color::White)),
            Span::raw(format!("  {}", n.etype)),
        ];
        if let Some(p) = n.parents.first() {
            spans.push(Span::styled(
                format!("  ◀─cause {}", hex8(p)),
                Style::default().fg(Color::Green),
            ));
        }
        dag_lines.push(Line::from(spans));
    }
    if facts.dag.is_empty() {
        dag_lines.push(Line::from(Span::styled(
            "  (en attente — [espace] pour lancer)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let dag_title = match focus {
        Focus::Tamper => " LOG CAUSAL — FALSIFICATION DÉTECTÉE ",
        _ => " LOG CAUSAL — le DAG se construit en direct ",
    };
    f.render_widget(
        Paragraph::new(dag_lines).block(Block::default().borders(Borders::ALL).title(dag_title)),
        body[1],
    );

    // Couche preuve [d] — recouvre le DAG par les détails vérifiables du moment.
    if drill {
        let dl = drill_lines(log, focus, review, verdict, tamper);
        // Clear : un Block ne fait que re-styler les cellules (les symboles du DAG
        // resteraient visibles) ; Clear remet toute la zone à blanc avant le rendu.
        f.render_widget(Clear, body[1]);
        f.render_widget(
            Paragraph::new(dl)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" PREUVE — [d] pour revenir au DAG "),
                )
                // wrap : les rapports LLM (causes) peuvent dépasser la largeur du panneau.
                .wrap(Wrap { trim: true }),
            body[1],
        );
    }

    // Ligne narrative (registre investisseur) + raccourcis.
    let narrative = match focus {
        Focus::Tamper => match tamper {
            Some(t) => format!(
                "▶ J'ai falsifié 1 octet du rapport : hash stocké {} ≠ recalculé {}. Le juge pointe dans le vide → détecté. [t] rétablir.",
                hex8(&t.target),
                hex8(&t.recomputed)
            ),
            None => "▶ Falsification annulée.".to_string(),
        },
        Focus::Rollback => {
            if facts.memo_rolled {
                "▶ État de l'agent mémoire annulé jusqu'au tour 1 — annulation atomique, sans état intermédiaire (P2).".to_string()
            } else {
                "▶ Rollback demandé sur l'agent mémoire…".to_string()
            }
        }
        Focus::Intrus => {
            if facts.rogue_denied {
                let what = facts.rogue_last.clone().unwrap_or_default();
                format!("▶ Intrus BLOQUÉ à la frontière — {what}. Refus tracé (CapabilityDenied 0x14). Son code n'y peut rien (P4).")
            } else {
                "▶ L'agent intrus tente un accès hors de son périmètre…".to_string()
            }
        }
        Focus::Pipeline => match step {
            Step::Start => "▶ Deux IA vont collaborer sur une revue de code. [espace] pour lancer.".to_string(),
            Step::ReviewerRunning => "▶ Le premier agent analyse le code… (inférence en rejeu).".to_string(),
            Step::ReviewerDone => {
                let n = review.as_ref().map(|(t, _)| t.lines().count()).unwrap_or(0);
                format!("▶ Rapport reçu ({n} remarques). [espace] : le transmettre au juge — l'arête sera un hash.")
            }
            Step::JudgeRunning => "▶ Le second agent évalue le rapport du premier…".to_string(),
            Step::Done => {
                let label = verdict
                    .as_ref()
                    .map(|(t, _)| if t.to_uppercase().contains("REJECT") { "REJECT" } else { "APPROVE" })
                    .unwrap_or("—");
                if llm_wrong {
                    format!("▶ Le LLM a {label} un code vulnérable. Le système ne corrige PAS la décision — il la rend traçable et attribuable (judge, dans le log [d]). Fiabilité sémantique du LLM = hors objet.")
                } else {
                    format!("▶ Verdict : {label}. Le lien entre les décisions EST une empreinte. Essayez [t] [r] [x].")
                }
            }
        },
    };
    let keys = "[espace] avancer  [t] falsifier  [r] rollback  [x] intrus  [d] preuve  [q] quitter";
    let foot = vec![
        Line::from(Span::styled(narrative, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(
        Paragraph::new(foot).block(Block::default().borders(Borders::ALL)),
        root[2],
    );
}

fn agent_line(name: &str, name_color: Color, state: &str, state_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{name:<9} "),
            Style::default().fg(name_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(state.to_string(), Style::default().fg(state_color)),
    ])
}

// ── Bootstrap commun + dispatch des scènes ──────────────────────────────────────

/// Répertoire de travail STABLE (et non /tmp horodaté) : le vérificateur tiers
/// `log-verify` sait ainsi où regarder sans copier-coller. Wipé au démarrage → chaque
/// run repart propre. L'écrivain (et lui seul) gère ce répertoire.
fn bootstrap() -> (Arc<ContentStore>, Arc<CausalLog>, wasmtime::Engine) {
    let work = PathBuf::from("demo-work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let cache = Cache::new_lru_cache(64 * 1024 * 1024);
    let store = Arc::new(ContentStore::open(&work.join("store"), Some(cache.clone())).unwrap());
    let log = Arc::new(CausalLog::open(&work.join("log"), Some(cache)).unwrap());
    (store, log, make_engine())
}

fn load_example(eng: &wasmtime::Engine, name: &str) -> wasmtime::Module {
    load_module_from_file(
        eng,
        Path::new(&format!(
            "target/wasm32-unknown-unknown/release/examples/{name}.wasm"
        )),
    )
    .unwrap_or_else(|_| panic!("{name}.wasm manquant — compiler l'exemple agent-sdk (voir en-tête)"))
}

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Effects,
    MissionResume,
    Incident,
    Swarm,
}

#[tokio::main]
async fn main() {
    // CLI : `demo-tui [--scene effects|mission-resume|incident] [--live] [modèle]
    //                 [--code <fichier>] [--llm-wrong]`.
    //   --scene <s>   : choix de scène (défaut effects).
    //   --code <f>    : (effects) le reviewer relit le code de <f> — public.
    //   --llm-wrong   : (effects) rejeu d'un LLM qui RATE la faille — honnêteté.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut live = false;
    let mut llm_wrong = false;
    let mut model = "llama3.2:3b".to_string();
    let mut code_path: Option<String> = None;
    let mut scene = Scene::Effects;
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--live" => live = true,
            "--llm-wrong" => llm_wrong = true,
            "--scene" => {
                i += 1;
                scene = match raw.get(i).map(|s| s.as_str()) {
                    Some("effects") => Scene::Effects,
                    Some("mission-resume") => Scene::MissionResume,
                    Some("incident") => Scene::Incident,
                    Some("swarm") => Scene::Swarm,
                    Some(o) => {
                        eprintln!("--scene inconnu : {o} (effects|mission-resume|incident|swarm)");
                        std::process::exit(2);
                    }
                    None => {
                        eprintln!("--scene requiert un nom");
                        std::process::exit(2);
                    }
                };
            }
            "--code" => {
                i += 1;
                code_path = raw.get(i).cloned();
            }
            other if !other.starts_with("--") => model = other.to_string(),
            _ => {}
        }
        i += 1;
    }

    match scene {
        Scene::Effects => scene_effects(live, llm_wrong, model, code_path).await,
        Scene::MissionResume => scene_mission_resume(live, model).await,
        Scene::Incident => scene_incident(live, model).await,
        Scene::Swarm => scene_swarm().await,
    }
}

// ── Scène effects : câblage runtime (miroir de code_review_runner) ──────────────

async fn scene_effects(live: bool, llm_wrong: bool, model: String, code_path: Option<String>) {
    let (store, log, eng) = bootstrap();

    let wasm_reviewer = load_example(&eng, "code_reviewer");
    let wasm_judge = load_example(&eng, "severity_judge");
    let wasm_memo = load_example(&eng, "multi_turn");
    let wasm_rogue = load_example(&eng, "data_accessor");

    // Code à soumettre au reviewer : fichier fourni par le public, sinon snippet câblé.
    let code = match &code_path {
        Some(p) => std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("--code : lecture de {p} impossible : {e}");
            std::process::exit(2);
        }),
        None => CODE_SNIPPET.to_string(),
    };

    // En --llm-wrong (rejeu), on injecte un reviewer qui rate la faille + un juge qui approuve.
    let (review_text, verdict_text) = if llm_wrong {
        (REVIEW_WRONG, VERDICT_WRONG)
    } else {
        (REVIEW_CANNED, VERDICT_CANNED)
    };
    let mut responses: HashMap<AgentId, String> = HashMap::new();
    responses.insert(ID_REVIEWER, review_text.to_string());
    responses.insert(ID_JUDGE, verdict_text.to_string());
    responses.insert(ID_MEMO, MEMO_CANNED.to_string());
    let backend = if live {
        DemoBackend::Ollama(OllamaBackend {
            model,
            endpoint: "http://localhost:11434".to_string(),
        })
    } else {
        DemoBackend::Canned(CannedBackend {
            responses: Arc::new(responses),
            delay_ms: 700,
        })
    };

    let pool = Arc::new(InferencePool::new_with_queue_params(2, 16, 30_000, backend));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    // Cap accordée à l'intrus : "reports" uniquement (pas "confidential/").
    let cap_id: u64 = {
        let mut lock = caps.lock().unwrap();
        lock.grant_root(
            ID_ROGUE,
            Permissions { read: true, write: true, execute: false, delegate: false },
            "reports".to_string(),
        )
    };

    // Helper de spawn (miroir du constructeur des runners).
    macro_rules! spawn_agent {
        ($wasm:expr, $id:expr, $granted:expr, $profile:expr, $cap:expr) => {{
            let (tx, rx) = tokio::sync::mpsc::channel::<Message>(8);
            tokio::spawn(os_poc_runtime::actor::run_loop(
                ActorInstance::new_precompiled_with_inference_and_profile(
                    &eng,
                    $wasm,
                    $id,
                    Arc::clone(&store),
                    Arc::clone(&log),
                    Arc::clone(&caps),
                    $granted,
                    SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
                    0,
                    InferencePool::as_infer_fn_with_class(Arc::clone(&pool), $cap),
                    $profile,
                )
                .await
                .expect("actor"),
                rx,
            ));
            tx
        }};
    }

    let tx_reviewer = spawn_agent!(&wasm_reviewer, ID_REVIEWER, vec![], AgentProfile::Batch, PriorityClass::Foreground);
    let tx_judge = spawn_agent!(&wasm_judge, ID_JUDGE, vec![], AgentProfile::Batch, PriorityClass::Foreground);
    let tx_memo = spawn_agent!(&wasm_memo, ID_MEMO, vec![], AgentProfile::Batch, PriorityClass::Foreground);
    let tx_rogue = spawn_agent!(&wasm_rogue, ID_ROGUE, vec![cap_id], AgentProfile::Algo, PriorityClass::Foreground);

    // Pré-alimenter l'agent mémoire pour que [r] ait un état à annuler (seq ≥ 2).
    let _ = tx_memo.send(Message::data(b"Retiens : le projet s'appelle NovOS.".to_vec())).await;
    let _ = tx_memo.send(Message::data(b"Retiens : la cible est seL4.".to_vec())).await;

    let log_ui = Arc::clone(&log);
    let senders = Senders {
        reviewer: tx_reviewer,
        judge: tx_judge,
        memo: tx_memo,
        rogue: tx_rogue,
    };
    let _ = tokio::task::spawn_blocking(move || {
        run_ui(log_ui, senders, cap_id, live, code, llm_wrong);
    })
    .await;
}

// ════════════════════════════════════════════════════════════════════════════════
//  Helpers partagés mission/incident (projection log + spawn d'acteur)
// ════════════════════════════════════════════════════════════════════════════════

/// Attend la prochaine ActionResult d'un agent au-delà de `after` (polling du log).
async fn wait_action_result(
    log: &CausalLog,
    id: &AgentId,
    after: usize,
    secs: u64,
) -> Option<(String, [u8; 32])> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let ids = log.query_by_agent_range(id, None, None).unwrap_or_default();
        for aid in ids.iter().skip(after) {
            let Ok(Some(e)) = log.get(aid) else { continue };
            let Some(pb) = e.emit_payload else { continue };
            let Ok(env) = EmitEnvelope::from_msgpack(&pb) else { continue };
            if env.emit_type == EmitType::ActionResult as u8 {
                return Some((String::from_utf8_lossy(&env.payload).trim().to_string(), *aid));
            }
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
    }
}

/// Toutes les ActionResult d'un agent, dans l'ordre — source de vérité de reprise.
fn read_action_results(log: &CausalLog, id: &AgentId) -> Vec<(String, [u8; 32])> {
    log.query_by_agent_range(id, None, None)
        .unwrap_or_default()
        .iter()
        .filter_map(|aid| {
            let e = log.get(aid).ok()??;
            let pb = e.emit_payload?;
            let env = EmitEnvelope::from_msgpack(&pb).ok()?;
            (env.emit_type == EmitType::ActionResult as u8)
                .then(|| (String::from_utf8_lossy(&env.payload).trim().to_string(), *aid))
        })
        .collect()
}

/// Message d'étape : contexte des étapes précédentes (séparé par "\n---\n") + instruction.
fn build_step_msg(completed: &[(String, [u8; 32])], instruction: &str) -> Vec<u8> {
    use std::io::Write as _;
    let mut msg = Vec::new();
    if !completed.is_empty() {
        for (i, (result, _)) in completed.iter().enumerate() {
            let _ = write!(msg, "Step {} result: {}\n", i + 1, result);
        }
        msg.extend_from_slice(b"\n---\n");
    }
    msg.extend_from_slice(instruction.as_bytes());
    msg
}

/// Spawne un acteur précompilé sur le pool de la démo, renvoie son sender.
async fn spawn_demo_actor(
    eng: &wasmtime::Engine,
    wasm: &wasmtime::Module,
    id: AgentId,
    store: Arc<ContentStore>,
    log: Arc<CausalLog>,
    caps: Arc<Mutex<CapabilityStore>>,
    pool: Arc<InferencePool<DemoBackend>>,
    class: PriorityClass,
) -> Sender<Message> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
    tokio::spawn(os_poc_runtime::actor::run_loop(
        ActorInstance::new_precompiled_with_inference_and_profile(
            eng,
            wasm,
            id,
            store,
            log,
            caps,
            vec![],
            SESSION_DEFAULT_VALIDATION_TIMEOUT_MS,
            0,
            InferencePool::as_infer_fn_with_class(pool, class),
            AgentProfile::Batch,
        )
        .await
        .expect("actor"),
        rx,
    ));
    tx
}

#[derive(Clone, Copy, PartialEq)]
enum StepStatus {
    Pending,
    Running,
    Done,
}

fn status_label(s: StepStatus, live: bool) -> (&'static str, Color) {
    match s {
        StepStatus::Pending => ("○ en attente", Color::DarkGray),
        StepStatus::Running => {
            if live {
                ("● infère… (Ollama)", Color::Cyan)
            } else {
                ("● infère… (rejeu)", Color::Cyan)
            }
        }
        StepStatus::Done => ("✓ committé au log", Color::Green),
    }
}

/// En-tête commun : titre de scène + mode + régime + substrat (honnêteté à l'écran).
fn header_line(title: &str, live: bool, regime: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {title} "),
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        if live {
            Span::styled(
                "mode: LIVE (Ollama)",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled("mode: rejeu", Style::default().fg(Color::Magenta))
        },
        Span::raw(" · "),
        Span::styled(regime.to_string(), Style::default().fg(Color::Green)),
        Span::raw(" · "),
        Span::styled("substrat: Linux", Style::default().fg(Color::DarkGray)),
    ])
}

/// Rendu d'un DAG (nœuds ●── agent hash etype ◀─cause) — style commun aux scènes.
fn dag_lines(dag: &[Node], genesis: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!("● {genesis} (genesis)"),
        Style::default().fg(Color::DarkGray),
    ))];
    for n in dag {
        let mut spans = vec![
            Span::styled("●── ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("{:<11} ", n.agent),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(hex8(&n.hash), Style::default().fg(Color::White)),
            Span::raw(format!("  {}", n.etype)),
        ];
        if let Some(p) = n.parents.first() {
            spans.push(Span::styled(
                format!("  ◀─cause {}", hex8(p)),
                Style::default().fg(Color::Green),
            ));
        }
        lines.push(Line::from(spans));
    }
    if dag.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (en attente — [espace] pour lancer)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn footer(narrative: String, limit: &str, keys: &str) -> Paragraph<'static> {
    let lines = vec![
        Line::from(Span::styled(
            narrative,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("⚠ {limit}"),
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(keys.to_string(), Style::default().fg(Color::DarkGray))),
    ];
    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: true })
}

// ════════════════════════════════════════════════════════════════════════════════
//  Scène mission-resume : un agent accomplit une tâche, reprise après interruption
//  Propriété : P3 (traçabilité) — le log est la source de vérité des résultats émis.
//  PAS P1a (aucune RAM mesurée). PAS P6 (interruption simulée, pas un crash). Régime R1.
// ════════════════════════════════════════════════════════════════════════════════

const MISSION_AGENT: AgentId = *b"task-agent000000";
const MISSION_INTERRUPT_AFTER: usize = 1; // interruption après l'étape 2 (0-indexée)

const MISSION_STEPS: &[(&str, &str)] = &[
    ("Étape 1/4 — Risques",
     "Identify the 3 main risks of deploying an AI agent in production. One sentence each."),
    ("Étape 2/4 — Mitigations",
     "For each risk above, propose one concrete mitigation. One sentence each."),
    ("Étape 3/4 — Rollback",
     "Define 3 observable conditions under which the deployment should be rolled back automatically."),
    ("Étape 4/4 — Synthèse",
     "Summarize the full deployment plan in exactly 3 bullet points, using the previous steps."),
];

// Réponses rejeu (une par étape, consommées dans l'ordre par SeqBackend).
const MISSION_CANNED: &[&str] = &[
    "Risk 1: silent quality drift as inputs shift. Risk 2: unbounded tool actions with side effects. Risk 3: cost/latency spikes under load.",
    "Mitigation 1: shadow-eval against a golden set before promote. Mitigation 2: capability-scoped tools with per-action audit. Mitigation 3: bounded concurrency + budget caps.",
    "Rollback if: (a) eval score drops >5% vs baseline, (b) audited denied-action rate exceeds threshold, (c) p99 latency breaches SLA for 5 min.",
    "• Promote only after shadow-eval passes the golden set.\n• Run with scoped capabilities + full causal audit.\n• Auto-rollback on eval/audit/latency breach.",
];

#[derive(Clone)]
struct MissionStep {
    title: String,
    status: StepStatus,
    action_id: Option<[u8; 32]>,
    snippet: String,
}

#[derive(Clone)]
struct MissionState {
    steps: Vec<MissionStep>,
    phase: String,
    interrupted: bool,
    recovered: usize,
    done: bool,
}

async fn scene_mission_resume(live: bool, model: String) {
    let (store, log, eng) = bootstrap();
    let wasm = load_example(&eng, "task_step");

    let backend = if live {
        DemoBackend::Ollama(OllamaBackend {
            model,
            endpoint: "http://localhost:11434".to_string(),
        })
    } else {
        let mut q = std::collections::VecDeque::new();
        for r in MISSION_CANNED {
            q.push_back(r.to_string());
        }
        let mut map: HashMap<AgentId, std::collections::VecDeque<String>> = HashMap::new();
        map.insert(MISSION_AGENT, q);
        DemoBackend::Seq(SeqBackend {
            responses: Arc::new(Mutex::new(map)),
            delay_ms: 900,
        })
    };
    let pool = Arc::new(InferencePool::new_with_queue_params(1, 8, 30_000, backend));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let steps = MISSION_STEPS
        .iter()
        .map(|(t, _)| MissionStep {
            title: t.to_string(),
            status: StepStatus::Pending,
            action_id: None,
            snippet: String::new(),
        })
        .collect();
    let state = Arc::new(Mutex::new(MissionState {
        steps,
        phase: "prêt — [espace] pour lancer".to_string(),
        interrupted: false,
        recovered: 0,
        done: false,
    }));
    let start = Arc::new(tokio::sync::Notify::new());

    // Driver : exécute la mission, met à jour l'état partagé (guards relâchés avant await).
    {
        let state = Arc::clone(&state);
        let start = Arc::clone(&start);
        let log = Arc::clone(&log);
        let store = Arc::clone(&store);
        let caps = Arc::clone(&caps);
        let pool = Arc::clone(&pool);
        let eng = eng.clone();
        let wasm = wasm.clone();
        tokio::spawn(async move {
            start.notified().await;
            let mut completed: Vec<(String, [u8; 32])> = Vec::new();
            for (idx, (_, instruction)) in MISSION_STEPS.iter().enumerate() {
                // Interruption simulée : RAM effacée → on relit le log (source de vérité).
                if idx == MISSION_INTERRUPT_AFTER + 1 {
                    let from_log = read_action_results(&log, &MISSION_AGENT);
                    completed = from_log.clone();
                    {
                        let mut s = state.lock().unwrap();
                        s.interrupted = true;
                        s.recovered = from_log.len();
                        s.phase = format!(
                            "INTERRUPTION simulée — {} étape(s) relue(s) du log",
                            from_log.len()
                        );
                    }
                    tokio::time::sleep(Duration::from_millis(1800)).await;
                }
                {
                    let mut s = state.lock().unwrap();
                    s.steps[idx].status = StepStatus::Running;
                    s.phase = format!("Étape {}/{} en cours", idx + 1, MISSION_STEPS.len());
                }
                let msg = build_step_msg(&completed, instruction);
                let before = log
                    .query_by_agent_range(&MISSION_AGENT, None, None)
                    .unwrap_or_default()
                    .len();
                let tx = spawn_demo_actor(
                    &eng,
                    &wasm,
                    MISSION_AGENT,
                    Arc::clone(&store),
                    Arc::clone(&log),
                    Arc::clone(&caps),
                    Arc::clone(&pool),
                    PriorityClass::Foreground,
                )
                .await;
                let _ = tx.send(Message::data(msg)).await;
                if let Some((text, aid)) = wait_action_result(&log, &MISSION_AGENT, before, 240).await
                {
                    completed.push((text.clone(), aid));
                    let mut s = state.lock().unwrap();
                    s.steps[idx].status = StepStatus::Done;
                    s.steps[idx].action_id = Some(aid);
                    s.steps[idx].snippet =
                        text.lines().next().unwrap_or("").chars().take(60).collect();
                }
                drop(tx);
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
            let mut s = state.lock().unwrap();
            s.done = true;
            s.phase = "Mission complète".to_string();
        });
    }

    let log_ui = Arc::clone(&log);
    let _ = tokio::task::spawn_blocking(move || mission_ui(log_ui, state, start, live)).await;
}

fn mission_ui(
    log: Arc<CausalLog>,
    state: Arc<Mutex<MissionState>>,
    start: Arc<tokio::sync::Notify>,
    live: bool,
) {
    let mut terminal = ratatui::init();
    let mut drill = false;
    let mut started = false;
    loop {
        let snap = state.lock().unwrap().clone();
        let dag = collect_dag(&log, &[(MISSION_AGENT, "mission")]);
        if terminal.draw(|f| mission_draw(f, &snap, &dag, live, drill)).is_err() {
            break;
        }
        match event::poll(Duration::from_millis(120)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char(' ') => {
                                if !started {
                                    start.notify_one();
                                    started = true;
                                }
                            }
                            KeyCode::Char('d') => drill = !drill,
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    }
    ratatui::restore();
}

fn mission_draw(f: &mut Frame, s: &MissionState, dag: &[Node], live: bool, drill: bool) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(5)])
        .split(f.area());

    f.render_widget(
        Paragraph::new(header_line(
            "MISSION — tâche longue, reprise après interruption",
            live,
            "régime: R1 (effets — traçabilité P3)",
        )),
        root[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(root[1]);

    // Panneau ÉTAPES.
    let mut step_lines: Vec<Line> = Vec::new();
    for st in &s.steps {
        let (lbl, col) = status_label(st.status, live);
        step_lines.push(Line::from(vec![
            Span::styled(
                format!("{:<22} ", st.title),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(lbl.to_string(), Style::default().fg(col)),
        ]));
        if let Some(aid) = st.action_id {
            step_lines.push(Line::from(Span::styled(
                format!("    action_id {} · {}", hex8(&aid), st.snippet),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    if s.interrupted {
        step_lines.push(Line::from(""));
        step_lines.push(Line::from(Span::styled(
            format!(
                ">>> INTERRUPTION SIMULÉE — RAM effacée · {} étape(s) relue(s) du log <<<",
                s.recovered
            ),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        step_lines.push(Line::from(Span::styled(
            "    les étapes faites ne sont PAS recalculées (aucun appel LLM)",
            Style::default().fg(Color::Green),
        )));
    }
    f.render_widget(
        Paragraph::new(step_lines)
            .block(Block::default().borders(Borders::ALL).title(" MISSION — 4 étapes "))
            .wrap(Wrap { trim: true }),
        body[0],
    );

    // Panneau DAG (ou preuve [d]).
    if drill {
        let mut dl: Vec<Line> = vec![Line::from(Span::styled(
            "PREUVE — le log est la source de vérité des résultats émis".to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))];
        for (i, st) in s.steps.iter().enumerate() {
            if let Some(aid) = st.action_id {
                dl.push(Line::from(Span::styled(
                    format!("étape {} action_id :", i + 1),
                    Style::default().fg(Color::DarkGray),
                )));
                dl.push(Line::from(Span::styled(
                    format!("  {}", full_hex(&aid)),
                    Style::default().fg(Color::White),
                )));
            }
        }
        dl.push(Line::from(""));
        dl.push(Line::from(Span::styled(
            "→ à la reprise, ces ActionResult sont relues du log et réinjectées comme contexte.".to_string(),
            Style::default().fg(Color::Green),
        )));
        dl.push(Line::from(Span::styled(
            "→ l'état AUTORITAIRE reste le ContentStore (ADR-0027) ; le log est l'observable des résultats.".to_string(),
            Style::default().fg(Color::Gray),
        )));
        f.render_widget(Clear, body[1]);
        f.render_widget(
            Paragraph::new(dl)
                .block(Block::default().borders(Borders::ALL).title(" PREUVE — [d] pour le DAG "))
                .wrap(Wrap { trim: true }),
            body[1],
        );
    } else {
        f.render_widget(
            Paragraph::new(dag_lines(dag, "tâche"))
                .block(Block::default().borders(Borders::ALL).title(" LOG CAUSAL — étapes committées ")),
            body[1],
        );
    }

    let narrative = if s.done {
        "▶ Mission complète. Les étapes faites avant l'interruption n'ont jamais été refaites — relues du log.".to_string()
    } else if s.interrupted {
        format!("▶ INTERRUPTION : RAM effacée. Contexte relu du log ({} étapes), zéro recomputation.", s.recovered)
    } else if s.steps.iter().any(|st| st.status != StepStatus::Pending) {
        format!("▶ {}", s.phase)
    } else {
        "▶ Un agent va exécuter une mission en 4 étapes. [espace] pour lancer.".to_string()
    };
    f.render_widget(
        footer(
            narrative,
            "Interruption SIMULÉE (relecture du log, pas un kill process) → P3 traçabilité, PAS P6 ni durabilité.",
            "[espace] lancer   [d] preuve (action_ids)   [q] quitter",
        ),
        root[2],
    );
}

// ════════════════════════════════════════════════════════════════════════════════
//  Scène incident : triage multi-agent — fan-out 3 spécialistes → fan-in agrégateur
//  Propriété : P3 (traçabilité) B-light mono-tenant (ADR-0036). Régime R1.
// ════════════════════════════════════════════════════════════════════════════════

const INCIDENT_NODE: AgentId = *b"incident-node000";
const AGGREGATOR: AgentId = *b"aggregator-00000";

const INCIDENT_TEXT: &str = "\
[ALERT] Production — multiple simultaneous symptoms:\n\
- inference CPU 98% (normal 40%), EU region only\n\
- DB query latency 2400ms (normal 240ms, 10x)\n\
- auth errors 340/min (normal <5/min)\n\
- started 14 min ago, no recent deploy";

// (clé rôle, titre agent, question, réponse rejeu)
const SPECIALISTS: &[(&str, &str, &str, &str)] = &[
    ("infra", "infrastructure-specialist",
     "Analyze: CPU spike 98% on inference servers, EU only. Most likely cause? Check first?",
     "Likely a runaway retry storm or a hot model loaded on EU nodes only. Check autoscaler events and per-node request rate first."),
    ("db", "database-specialist",
     "Analyze: query latency spiked 10x, no recent deploy. Most likely cause? Check first?",
     "Latency 10x with no deploy points to lock contention or a missing index hit by a new query shape. Check slow-query log and active locks first."),
    ("security", "security-specialist",
     "Analyze: auth errors jumped to 340/min, EU only. Attack? Check first?",
     "The EU-localized auth error burst is consistent with credential stuffing. Check failed-login source IPs and rate-limit state first."),
];

const INCIDENT_NODE_CANNED: &str = "Incident recorded verbatim into the causal log (root node).";
const AGGREGATOR_CANNED: &str = "\
1. Root cause: an EU-localized retry/credential-stuffing storm overloading inference + DB.\n\
2. Immediate actions: • enable per-IP rate limiting • shed retry load on EU nodes • add the missing DB index.\n\
3. Escalation: Yes — security on-call, EU credential-stuffing pattern confirmed.";

#[derive(Clone)]
struct SpecView {
    role: String,
    status: StepStatus,
    action_id: Option<[u8; 32]>,
    snippet: String,
}

#[derive(Clone)]
struct IncidentState {
    phase: String,
    incident_id: Option<[u8; 32]>,
    specialists: Vec<SpecView>,
    report: Option<(String, [u8; 32])>,
    done: bool,
}

fn specialist_id(i: usize) -> AgentId {
    let mut id = *b"specialist-00000";
    id[11] = b'0' + i as u8;
    id
}

async fn scene_incident(live: bool, model: String) {
    let (store, log, eng) = bootstrap();
    let wasm_step = load_example(&eng, "task_step");
    let wasm_agg = load_example(&eng, "incident_aggregator");

    let backend = if live {
        DemoBackend::Ollama(OllamaBackend {
            model,
            endpoint: "http://localhost:11434".to_string(),
        })
    } else {
        let mut map: HashMap<AgentId, std::collections::VecDeque<String>> = HashMap::new();
        map.insert(INCIDENT_NODE, [INCIDENT_NODE_CANNED.to_string()].into());
        for (i, (_, _, _, resp)) in SPECIALISTS.iter().enumerate() {
            map.insert(specialist_id(i), [resp.to_string()].into());
        }
        map.insert(AGGREGATOR, [AGGREGATOR_CANNED.to_string()].into());
        DemoBackend::Seq(SeqBackend {
            responses: Arc::new(Mutex::new(map)),
            delay_ms: 900,
        })
    };
    // cap=3 : les 3 spécialistes infèrent en parallèle.
    let pool = Arc::new(InferencePool::new_with_queue_params(3, 8, 30_000, backend));
    let caps = Arc::new(Mutex::new(CapabilityStore::new()));

    let specialists = SPECIALISTS
        .iter()
        .map(|(role, _, _, _)| SpecView {
            role: role.to_string(),
            status: StepStatus::Pending,
            action_id: None,
            snippet: String::new(),
        })
        .collect();
    let state = Arc::new(Mutex::new(IncidentState {
        phase: "prêt — [espace] pour lancer".to_string(),
        incident_id: None,
        specialists,
        report: None,
        done: false,
    }));
    let start = Arc::new(tokio::sync::Notify::new());

    {
        let state = Arc::clone(&state);
        let start = Arc::clone(&start);
        let log = Arc::clone(&log);
        let store = Arc::clone(&store);
        let caps = Arc::clone(&caps);
        let pool = Arc::clone(&pool);
        let eng = eng.clone();
        tokio::spawn(async move {
            start.notified().await;

            // 1) Nœud incident = racine du DAG.
            {
                let mut s = state.lock().unwrap();
                s.phase = "Enregistrement de l'incident (racine du DAG)".to_string();
            }
            let before_inc = log
                .query_by_agent_range(&INCIDENT_NODE, None, None)
                .unwrap_or_default()
                .len();
            let tx_inc = spawn_demo_actor(
                &eng, &wasm_step, INCIDENT_NODE,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps),
                Arc::clone(&pool), PriorityClass::Supervisor,
            )
            .await;
            let _ = tx_inc
                .send(Message::data(
                    format!("\n---\nRecord this incident verbatim:\n{INCIDENT_TEXT}").into_bytes(),
                ))
                .await;
            let inc_action_id = match wait_action_result(&log, &INCIDENT_NODE, before_inc, 120).await
            {
                Some((_, aid)) => aid,
                None => return,
            };
            drop(tx_inc);
            {
                let mut s = state.lock().unwrap();
                s.incident_id = Some(inc_action_id);
                s.phase = "Fan-out : 3 spécialistes en parallèle".to_string();
            }

            // 2) Fan-out : 3 spécialistes liés causalement à l'incident.
            let mut handles = Vec::new();
            for (i, (role, title, question, _)) in SPECIALISTS.iter().enumerate() {
                let sid = specialist_id(i);
                {
                    let mut s = state.lock().unwrap();
                    s.specialists[i].status = StepStatus::Running;
                }
                let before = log.query_by_agent_range(&sid, None, None).unwrap_or_default().len();
                let tx = spawn_demo_actor(
                    &eng, &wasm_step, sid,
                    Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps),
                    Arc::clone(&pool), PriorityClass::Foreground,
                )
                .await;
                let msg = format!("You are a {title}.\nContext:\n{INCIDENT_TEXT}\n---\n{question}");
                let _ = tx.send(Message::caused(msg.into_bytes(), inc_action_id)).await;
                handles.push((i, *role, sid, before, tx));
            }

            // 3) Collecte parallèle des analyses.
            let mut analyses: Vec<(usize, String, [u8; 32])> = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while analyses.len() < handles.len() {
                tokio::time::sleep(Duration::from_millis(250)).await;
                for (i, _role, sid, before, _) in &handles {
                    if analyses.iter().any(|(j, _, _)| j == i) {
                        continue;
                    }
                    if let Some((text, aid)) = {
                        let ids = log.query_by_agent_range(sid, None, None).unwrap_or_default();
                        let mut found = None;
                        for a in ids.iter().skip(*before) {
                            if let Ok(Some(e)) = log.get(a) {
                                if let Some(pb) = e.emit_payload {
                                    if let Ok(env) = EmitEnvelope::from_msgpack(&pb) {
                                        if env.emit_type == EmitType::ActionResult as u8 {
                                            found = Some((
                                                String::from_utf8_lossy(&env.payload).trim().to_string(),
                                                *a,
                                            ));
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        found
                    } {
                        analyses.push((*i, text.clone(), aid));
                        let mut s = state.lock().unwrap();
                        s.specialists[*i].status = StepStatus::Done;
                        s.specialists[*i].action_id = Some(aid);
                        s.specialists[*i].snippet =
                            text.lines().next().unwrap_or("").chars().take(50).collect();
                    }
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }
            drop(handles);

            // 4) Fan-in : agrégateur lié aux 3 analyses + FINALIZE lié à l'incident.
            {
                let mut s = state.lock().unwrap();
                s.phase = format!("Fan-in : agrégation de {} analyses", analyses.len());
            }
            let tx_agg = spawn_demo_actor(
                &eng, &wasm_agg, AGGREGATOR,
                Arc::clone(&store), Arc::clone(&log), Arc::clone(&caps),
                Arc::clone(&pool), PriorityClass::Supervisor,
            )
            .await;
            for (i, analysis, spec_aid) in &analyses {
                let role = SPECIALISTS[*i].0;
                let msg = format!("REPORT:{role}:{analysis}");
                let _ = tx_agg.send(Message::caused(msg.into_bytes(), *spec_aid)).await;
            }
            let _ = tx_agg.send(Message::caused(b"FINALIZE".to_vec(), inc_action_id)).await;
            let before_agg = log.query_by_agent_range(&AGGREGATOR, None, None).unwrap_or_default().len();
            if let Some((report, rid)) = wait_action_result(&log, &AGGREGATOR, before_agg, 240).await {
                let mut s = state.lock().unwrap();
                s.report = Some((report, rid));
            }
            drop(tx_agg);
            let mut s = state.lock().unwrap();
            s.done = true;
            s.phase = "Rapport d'incident produit".to_string();
        });
    }

    let log_ui = Arc::clone(&log);
    let _ = tokio::task::spawn_blocking(move || incident_ui(log_ui, state, start, live)).await;
}

fn incident_ui(
    log: Arc<CausalLog>,
    state: Arc<Mutex<IncidentState>>,
    start: Arc<tokio::sync::Notify>,
    live: bool,
) {
    let mut terminal = ratatui::init();
    let mut drill = false;
    let mut started = false;
    let dag_ids: Vec<(AgentId, &str)> = {
        let mut v = vec![(INCIDENT_NODE, "incident")];
        // labels statiques (les &str doivent vivre assez longtemps : ce sont des littéraux).
        v.push((specialist_id(0), "infra"));
        v.push((specialist_id(1), "db"));
        v.push((specialist_id(2), "security"));
        v.push((AGGREGATOR, "agrégateur"));
        v
    };
    loop {
        let snap = state.lock().unwrap().clone();
        let dag = collect_dag(&log, &dag_ids);
        if terminal.draw(|f| incident_draw(f, &snap, &dag, live, drill)).is_err() {
            break;
        }
        match event::poll(Duration::from_millis(120)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char(' ') => {
                                if !started {
                                    start.notify_one();
                                    started = true;
                                }
                            }
                            KeyCode::Char('d') => drill = !drill,
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    }
    ratatui::restore();
}

fn incident_draw(f: &mut Frame, s: &IncidentState, dag: &[Node], live: bool, drill: bool) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(5)])
        .split(f.area());

    f.render_widget(
        Paragraph::new(header_line(
            "INCIDENT — fan-out 3 spécialistes / fan-in agrégateur",
            live,
            "régime: R1 (effets — DAG causal B-light)",
        )),
        root[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(root[1]);

    // Panneau ACTEURS.
    let mut lines: Vec<Line> = Vec::new();
    let inc_state = if s.incident_id.is_some() { "✓ enregistré (racine)" } else { "○ en attente" };
    lines.push(Line::from(vec![
        Span::styled("incident      ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::styled(inc_state.to_string(), Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Spécialistes (fan-out parallèle) :",
        Style::default().fg(Color::Gray),
    )));
    for sp in &s.specialists {
        let (lbl, col) = status_label(sp.status, live);
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<10} ", sp.role),
                Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            ),
            Span::styled(lbl.to_string(), Style::default().fg(col)),
        ]));
        if let Some(aid) = sp.action_id {
            lines.push(Line::from(Span::styled(
                format!("     {} · {}", hex8(&aid), sp.snippet),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    lines.push(Line::from(""));
    let agg_state = if s.report.is_some() {
        ("✓ rapport synthétisé", Color::Green)
    } else if s.specialists.iter().all(|sp| sp.status == StepStatus::Done) && s.incident_id.is_some() {
        ("● synthèse… (fan-in)", Color::Cyan)
    } else {
        ("○ attend les analyses", Color::DarkGray)
    };
    lines.push(Line::from(vec![
        Span::styled("agrégateur    ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(agg_state.0.to_string(), Style::default().fg(agg_state.1)),
    ]));
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" ACTEURS "))
            .wrap(Wrap { trim: true }),
        body[0],
    );

    // Panneau DAG / rapport (preuve [d]).
    if drill {
        let mut dl: Vec<Line> = vec![Line::from(Span::styled(
            "PREUVE — DAG causal : fan-in à 3 parents".to_string(),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))];
        if let Some((report, rid)) = &s.report {
            dl.push(Line::from(Span::styled(
                "rapport final action_id :".to_string(),
                Style::default().fg(Color::DarkGray),
            )));
            dl.push(Line::from(Span::styled(
                format!("  {}", full_hex(rid)),
                Style::default().fg(Color::White),
            )));
            dl.push(Line::from(""));
            for l in report.lines() {
                dl.push(Line::from(Span::styled(
                    format!("  {}", l.trim_end()),
                    Style::default().fg(Color::Gray),
                )));
            }
        } else {
            dl.push(Line::from(Span::styled(
                "  (rapport non encore produit)".to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        }
        f.render_widget(Clear, body[1]);
        f.render_widget(
            Paragraph::new(dl)
                .block(Block::default().borders(Borders::ALL).title(" PREUVE — [d] pour le DAG "))
                .wrap(Wrap { trim: true }),
            body[1],
        );
    } else {
        f.render_widget(
            Paragraph::new(dag_lines(dag, "incident"))
                .block(Block::default().borders(Borders::ALL).title(" LOG CAUSAL — le DAG se construit ")),
            body[1],
        );
    }

    let narrative = if s.done {
        "▶ Rapport d'incident produit. Chaque analyse partielle est dans le log, attribuée et rejouable.".to_string()
    } else if s.report.is_none() && s.specialists.iter().all(|sp| sp.status == StepStatus::Done) && s.incident_id.is_some() {
        "▶ Fan-in : l'agrégateur synthétise les 3 analyses (chacune liée par un hash).".to_string()
    } else if s.incident_id.is_some() {
        "▶ Fan-out : 3 spécialistes analysent en parallèle, chacun lié causalement à l'incident.".to_string()
    } else {
        "▶ Un incident, 3 symptômes simultanés. [espace] pour lancer le triage parallèle.".to_string()
    };
    f.render_widget(
        footer(
            narrative,
            "DAG B-light mono-tenant (ADR-0036) : liens vérifiés en existence O(1), sans capability cross-agent. tamper-evident ≠ tamper-proof.",
            "[espace] lancer   [d] preuve (DAG + rapport)   [q] quitter",
        ),
        root[2],
    );
}

// ════════════════════════════════════════════════════════════════════════════════
//  Scène swarm : MÉCANISME d'ordonnancement (PAS une mesure de densité).
//  Acte 1 — admission bornée (C2, IoAdmissionQueue) : in-flight ≤ cap garanti par
//           sémaphore ; le surplus attend, rien n'est perdu.
//  Acte 2 — densité : éviction → dormant (Scheduler), réveil depuis le snapshot.
//  Compteurs RÉELS uniquement (in_flight, dormant_count). Backend simulé.
//  Garde-fou architecte : aucune densité revendiquée, aucun ~100 agents/s, aucune
//  arithmétique RAM. N à l'écran ≠ N soutenables.
// ════════════════════════════════════════════════════════════════════════════════

const SWARM_N: usize = 14; // agents du burst d'admission
const SWARM_CAP: usize = 4; // borne dure d'admission concurrente (C2)
const SWARM_M: usize = 10; // agents persistants pour l'acte éviction/réveil

#[derive(Clone, Copy, PartialEq)]
enum SwarmPhase {
    Idle,
    Burst,
    Density,
}

enum SwarmCmd {
    Evict,
    Wake,
}

#[derive(Clone)]
struct SwarmState {
    phase: SwarmPhase,
    label: String,
    burst_total: usize,
    burst_done: usize,
    cap: usize,
    in_flight: u32,
    waiting: usize,
    pop_total: usize,
    dormant: usize,
    cells: Vec<u8>, // 0=attente, 1=admis/travaille, 2=fini, 3=dormant
}

async fn scene_swarm() {
    let (store, log, eng) = bootstrap();

    let state = Arc::new(Mutex::new(SwarmState {
        phase: SwarmPhase::Idle,
        label: "prêt — [espace] pour lancer".to_string(),
        burst_total: SWARM_N,
        burst_done: 0,
        cap: SWARM_CAP,
        in_flight: 0,
        waiting: 0,
        pop_total: 0,
        dormant: 0,
        cells: vec![0u8; SWARM_N],
    }));
    let start = Arc::new(tokio::sync::Notify::new());
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<SwarmCmd>();

    {
        let state = Arc::clone(&state);
        let start = Arc::clone(&start);
        let store = Arc::clone(&store);
        let log = Arc::clone(&log);
        let eng = eng.clone();
        tokio::spawn(async move {
            start.notified().await;

            // ── Acte 1 : admission bornée (C2). in_flight ≤ cap (sémaphore). ─────
            {
                let mut s = state.lock().unwrap();
                s.phase = SwarmPhase::Burst;
                s.label = "Acte 1 — admission bornée (C2) : in-flight ≤ cap".to_string();
                s.cells = vec![0u8; SWARM_N];
            }
            let io_queue = Arc::new(IoAdmissionQueue::new(SWARM_CAP, SWARM_CAP * 8));
            let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let cells = Arc::new(Mutex::new(vec![0u8; SWARM_N]));

            for i in 0..SWARM_N {
                let mut id = [0u8; 16];
                id[14] = 0xA0;
                id[15] = i as u8;
                let io_queue = Arc::clone(&io_queue);
                let done = Arc::clone(&done);
                let cells = Arc::clone(&cells);
                tokio::spawn(async move {
                    if let Ok(permit) = io_queue.acquire(id, PriorityClass::Foreground, None).await {
                        {
                            cells.lock().unwrap()[i] = 1; // admis : occupe un slot borné
                        }
                        // Travail borné pendant la tenue du permit (préchargement + inférence simulés).
                        tokio::time::sleep(Duration::from_millis(900)).await;
                        drop(permit); // libère le slot → un agent en attente est admis
                    }
                    {
                        cells.lock().unwrap()[i] = 2;
                    }
                    done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                });
            }

            loop {
                tokio::time::sleep(Duration::from_millis(120)).await;
                let d = done.load(std::sync::atomic::Ordering::SeqCst);
                let st = io_queue.stats();
                {
                    let mut s = state.lock().unwrap();
                    s.burst_done = d;
                    s.in_flight = io_queue.in_flight();
                    s.waiting = st.waiting.iter().sum();
                    s.cells = cells.lock().unwrap().clone();
                }
                if d >= SWARM_N {
                    break;
                }
            }

            // ── Acte 2 : densité — éviction / réveil (Scheduler réel). ──────────
            let module = match wasmtime::Module::new(&eng, AGENT_WAT) {
                Ok(m) => m,
                Err(_) => return,
            };
            let mut scheduler = Scheduler::new();
            let mut ids: Vec<AgentId> = Vec::new();
            // Garder les senders vivants : si on les drop, le canal se ferme et l'agent
            // termine → plus rien à évincer (cf. s11_runner qui conserve `senders`).
            let mut _senders = Vec::new();
            for j in 0..SWARM_M {
                let mut id = [0u8; 16];
                id[14] = 0xB0;
                id[15] = j as u8;
                let inst = match ActorInstance::new_precompiled(
                    &eng,
                    &module,
                    id,
                    Arc::clone(&store),
                    Arc::clone(&log),
                )
                .await
                {
                    Ok(inst) => inst,
                    Err(_) => return,
                };
                let tx = scheduler.register(inst);
                for k in 0..3u8 {
                    let _ = tx.send(Message::data(vec![k])).await; // état à préserver à l'éviction
                }
                ids.push(id);
                _senders.push(tx);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            {
                let mut s = state.lock().unwrap();
                s.phase = SwarmPhase::Density;
                s.label = "Acte 2 — densité : [e] évincer (→ dormant), [w] réveiller".to_string();
                s.pop_total = SWARM_M;
                s.dormant = 0;
                s.cells = vec![1u8; SWARM_M];
            }

            loop {
                match cmd_rx.try_recv() {
                    Ok(SwarmCmd::Evict) => {
                        if let Some(id) = ids.iter().find(|id| !scheduler.is_dormant(id)).copied() {
                            let _ = scheduler.evict_agent(&id).await;
                        }
                    }
                    Ok(SwarmCmd::Wake) => {
                        if let Some(id) = ids.iter().find(|id| scheduler.is_dormant(id)).copied() {
                            let _ = scheduler
                                .wake_agent(&id, &eng, &module, Arc::clone(&store), Arc::clone(&log))
                                .await;
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break, // UI quittée
                }
                {
                    let mut s = state.lock().unwrap();
                    s.dormant = scheduler.dormant_count();
                    s.cells = ids
                        .iter()
                        .map(|id| if scheduler.is_dormant(id) { 3u8 } else { 1u8 })
                        .collect();
                }
                tokio::time::sleep(Duration::from_millis(120)).await;
            }
        });
    }

    let _ = tokio::task::spawn_blocking(move || swarm_ui(state, start, cmd_tx)).await;
}

fn swarm_ui(
    state: Arc<Mutex<SwarmState>>,
    start: Arc<tokio::sync::Notify>,
    cmd_tx: std::sync::mpsc::Sender<SwarmCmd>,
) {
    let mut terminal = ratatui::init();
    let mut started = false;
    loop {
        let snap = state.lock().unwrap().clone();
        if terminal.draw(|f| swarm_draw(f, &snap)).is_err() {
            break;
        }
        match event::poll(Duration::from_millis(120)) {
            Ok(true) => {
                if let Ok(Event::Key(k)) = event::read() {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char(' ') => {
                                if !started {
                                    start.notify_one();
                                    started = true;
                                }
                            }
                            KeyCode::Char('e') => {
                                let _ = cmd_tx.send(SwarmCmd::Evict);
                            }
                            KeyCode::Char('w') => {
                                let _ = cmd_tx.send(SwarmCmd::Wake);
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(false) => {}
            Err(_) => break,
        }
    }
    ratatui::restore();
}

fn bar(used: usize, cap: usize) -> String {
    let used = used.min(cap);
    let filled: String = std::iter::repeat('▣').take(used).collect();
    let empty: String = std::iter::repeat('▢').take(cap.saturating_sub(used)).collect();
    format!("{filled}{empty}")
}

fn swarm_draw(f: &mut Frame, s: &SwarmState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(5)])
        .split(f.area());

    f.render_widget(
        Paragraph::new(header_line(
            "SWARM — ordonnancement borné + densité",
            false,
            "mécanisme d'ordonnancement (R2 non mesuré — backend simulé)",
        )),
        root[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(root[1]);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("phase : {}", s.label),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if s.phase != SwarmPhase::Density {
        lines.push(Line::from(Span::styled(
            "Admission bornée (C2) — sémaphore :",
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(vec![
            Span::raw("  in-flight "),
            Span::styled(bar(s.in_flight as usize, s.cap), Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {}/{}", s.in_flight, s.cap)),
        ]));
        lines.push(Line::from(Span::styled(
            format!("  en file d'attente : {}", s.waiting),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("burst : {}/{} terminés", s.burst_done, s.burst_total),
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(Span::styled(
            "→ borne DURE : au plus cap en vol ; le reste attend, rien perdu.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  (C1 inférence bornée = mécanisme jumeau, S5 ; non mesuré ici.)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("Population : {} agents", s.pop_total),
            Style::default().fg(Color::Gray),
        )));
        lines.push(Line::from(Span::styled(
            format!("  actifs   : {}", s.pop_total.saturating_sub(s.dormant)),
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(Span::styled(
            format!("  dormants : {}", s.dormant),
            Style::default().fg(Color::Magenta),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "→ un dormant sort de la mémoire active ;",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "  au réveil il reprend depuis son dernier snapshot (S11/S12).",
            Style::default().fg(Color::DarkGray),
        )));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" COMPTEURS (réels) "))
            .wrap(Wrap { trim: true }),
        body[0],
    );

    // Grille d'états de l'essaim.
    let glyph = |c: u8| match c {
        1 => Span::styled("● ", Style::default().fg(Color::Green)),
        2 => Span::styled("✓ ", Style::default().fg(Color::Blue)),
        3 => Span::styled("⏸ ", Style::default().fg(Color::Magenta)),
        _ => Span::styled("○ ", Style::default().fg(Color::DarkGray)),
    };
    let mut grid: Vec<Line> = Vec::new();
    let mut rowspans: Vec<Span> = Vec::new();
    for (i, c) in s.cells.iter().enumerate() {
        rowspans.push(glyph(*c));
        if (i + 1) % 7 == 0 {
            grid.push(Line::from(std::mem::take(&mut rowspans)));
        }
    }
    if !rowspans.is_empty() {
        grid.push(Line::from(rowspans));
    }
    grid.push(Line::from(""));
    grid.push(Line::from(Span::styled(
        "○ attente   ● admis/travaille   ✓ fini   ⏸ dormant",
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(
        Paragraph::new(grid).block(Block::default().borders(Borders::ALL).title(" ESSAIM ")),
        body[1],
    );

    let narrative = match s.phase {
        SwarmPhase::Idle => {
            "▶ Un essaim d'agents arrive. Le scheduler borne le travail concurrent. [espace].".to_string()
        }
        SwarmPhase::Burst => {
            "▶ Admission bornée (C2) : au plus cap en vol, le surplus attend — aucun agent perdu ni affamé.".to_string()
        }
        SwarmPhase::Density => {
            "▶ Densité : [e] évince un agent (→ dormant, hors mémoire active), [w] le réveille (reprend du snapshot).".to_string()
        }
    };
    f.render_widget(
        footer(
            narrative,
            "MÉCANISME, pas une mesure. N à l'écran ≠ N soutenables ; densité hébergée vs active (~70, cap 14/s, spec/07) distinctes, NON mesurées ici. Aucun ~100 agents/s.",
            "[espace] lancer   [e] évincer   [w] réveiller   [q] quitter",
        ),
        root[2],
    );
}
