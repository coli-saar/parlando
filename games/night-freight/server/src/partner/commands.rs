//! Typed chat commands that steer the scripted partner's own notes.
//!
//! This unit owns the small, deterministic command language of the scripted
//! partner. It has two halves that the partner calls in sequence:
//!
//! 1. [`parse_message`] reads one chat message and returns the commands it
//!    contains, a plain-words problem for every command-shaped segment it
//!    could not use, and the text of the segments that are *not* commands. The
//!    partner feeds that remainder to its readiness and fact cues, so
//!    "ready" and "what do you know?" keep working exactly as before.
//! 2. [`compile`] turns one parsed [`Command`] into [`NoteEdit`]s against the
//!    partner's current notes, resolving defaults (the only engine, the last
//!    stop) and wording the confirmation.
//!
//! The partner may rely on the following. Parsing never fails: unrecognized
//! text is returned in [`Parsed::rest`]. The parser reads only the chat text
//! and the shared station names, so it cannot disclose anything private. Compiling
//! reads only the partner's own notes. Neither half validates map facts such as
//! adjacency or order ids: the game does that and the partner reports its
//! rejection code with [`reject_words`].
//!
//! # Grammar
//!
//! Text is lower-cased and split into words; punctuation separates words and
//! extra words are ignored. A message may hold several commands separated by
//! `;` or a line break. A segment is a command when, after leading filler
//! words such as "please" or "ok", it starts with an engine id (`E1`) or with
//! one of the verbs below. Stops and ops are numbered from 0, as in the notes
//! shown by `show notes`; stop 0 is the engine's starting station.
//!
//! | Command | Example |
//! | --- | --- |
//! | append stops | `E1 to Dansville`, `E1 go Dansville Corning Bath` |
//! | truncate | `E1 cut from 2` |
//! | departure | `E1 stop 2 leave at 5`, `E1 stop 2 leave now`, `E1 stop 2 leave 1 after E2` |
//! | add an op at the end of a stop | `E1 stop 0 couple 2 boxcar` (any cars), `couple 1 boxcar empty`, `uncouple 1 boxcar oranges` (loaded), `load 2 oranges`, `process 1`, `deliver O1` |
//! | remove an op | `E1 stop 1 remove op 0` |
//! | heard arrival | `E1 stop 1 heard 6`, `E1 stop 1 heard none` |
//! | show / help | `show notes`, `help` |
//!
//! `stop` may name a station instead of a number (`E1 stop dansville deliver
//! O1`). Without a `stop` an op goes to the engine's last stop. Without an
//! engine id the command applies to the only engine of the notes.

use crate::plan::{Depart, LoadFilter, NoteEdit, Op, Plan};
use crate::world::{CarKind, Commodity};
use crate::Seat;

/// The stations of the shared map, for recognizing station names in chat.
#[derive(Clone, Debug, Default)]
pub struct Vocabulary {
    /// `(id, lower-cased display name)` per station.
    stations: Vec<(String, String)>,
}

impl Vocabulary {
    /// Builds the vocabulary from `(station id, display name)` pairs.
    pub fn new<'a>(stations: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            stations: stations
                .into_iter()
                .map(|(id, name)| (id.to_string(), name.to_lowercase()))
                .collect(),
        }
    }

    /// The station id a lower-cased word names, by id or display name.
    fn station(&self, word: &str) -> Option<&str> {
        self.stations
            .iter()
            .find(|(id, name)| id == word || name == word)
            .map(|(id, _)| id.as_str())
    }

    /// The station ids, for help texts.
    fn ids(&self) -> Vec<&str> {
        self.stations.iter().map(|(id, _)| id.as_str()).collect()
    }
}

/// Which stop of an engine a command addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopRef {
    /// The stop with this index.
    Index(usize),
    /// The first stop at this station.
    Station(String),
    /// The engine's last stop.
    Last,
}

/// One parsed command. Engines are `None` when the chat did not name one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// List the commands.
    Help,
    /// Reply with the partner's notes.
    ShowNotes,
    /// Append stops at the given stations, in order.
    Route {
        /// The engine.
        engine: Option<String>,
        /// Station ids.
        stations: Vec<String>,
    },
    /// Remove stops `from..`.
    Cut {
        /// The engine.
        engine: Option<String>,
        /// First stop index to remove.
        from: usize,
    },
    /// Set a stop's departure condition.
    Depart {
        /// The engine.
        engine: Option<String>,
        /// The stop.
        stop: StopRef,
        /// The condition.
        depart: Depart,
    },
    /// Insert an op at the end of a stop's op list.
    AddOp {
        /// The engine.
        engine: Option<String>,
        /// The stop.
        stop: StopRef,
        /// The op.
        op: Op,
    },
    /// Remove one op from a stop.
    RemoveOp {
        /// The engine.
        engine: Option<String>,
        /// The stop.
        stop: StopRef,
        /// Position in the stop's op list.
        index: usize,
    },
    /// Set or clear the heard-arrival annotation.
    Heard {
        /// The engine.
        engine: Option<String>,
        /// The stop.
        stop: StopRef,
        /// The hour, or `None` to clear.
        hour: Option<u8>,
    },
}

/// A command or the plain-words reason a command-shaped segment was unusable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A command to carry out.
    Command(Command),
    /// A reply to send instead.
    Problem(String),
}

/// The result of reading one chat message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    /// Commands and problems in the order the human wrote them.
    pub items: Vec<Item>,
    /// The segments that were not commands, for the readiness and fact cues.
    pub rest: String,
}

/// The outcome of reading one segment.
enum Segment {
    /// Not command-shaped.
    Other,
    /// A command.
    Command(Command),
    /// Command-shaped but unusable, with the reply.
    Problem(String),
}

/// Words skipped at the start of a segment.
const FILLERS: &[&str] = &[
    "please", "pls", "ok", "okay", "then", "now", "and", "so", "can", "could", "would", "you",
    "let", "s", "send", "have", "make", "tell", "set", "just", "also", "hey", "hi", "partner",
];

/// Verbs that start a command without an engine id.
const STARTERS: &[&str] = &[
    "couple", "uncouple", "load", "process", "deliver", "cut", "truncate", "remove", "leave",
    "heard", "go", "goto", "route", "add", "append", "visit", "via",
];

/// Words that append stops.
const ROUTE_VERBS: &[&str] = &["to", "go", "goto", "route", "add", "append", "visit", "via"];

/// Words that truncate the strip.
const CUT_VERBS: &[&str] = &["cut", "truncate", "trim"];

/// Words that remove an op.
const REMOVE_VERBS: &[&str] = &["remove", "delete", "rm"];

/// Words that set a departure.
const LEAVE_VERBS: &[&str] = &["leave", "leaves", "depart", "departs"];

/// Words that set the heard-arrival annotation.
const HEARD_VERBS: &[&str] = &["heard", "arrival", "arrives"];

/// Words that insert an op.
const OP_VERBS: &[&str] = &["couple", "uncouple", "load", "process", "deliver"];

/// The command summary the partner sends for `help`.
pub const HELP: &str = "My note commands (case does not matter, stops and ops count from 0, \
separate several commands with a semicolon): \
\"E1 to Dansville Corning\" appends stops, \"E1 cut from 2\" removes stop 2 and later, \
\"E1 stop 1 leave at 5\", \"leave now\" or \"leave 1 after E2\" sets a departure, \
\"E1 stop 0 couple 2 boxcar\" (any boxcars, empty ones first), \"couple 1 boxcar empty\", \
\"uncouple 1 boxcar oranges\" (boxcars loaded with oranges), \"load 2 oranges\", \"process 1\" \
and \"deliver O1\" add an op at the end of that stop, \"E1 stop 1 remove op 0\" removes one, \
\"E1 stop 1 heard 6\" jots an arrival hour, \"show notes\" lists my notes. \
Also \"ready\", \"wait\", and ask for my facts.";

/// Splits chat text into lower-case words; punctuation separates words.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The engine id (`"E1"`) a word names, if any.
fn engine_id(word: &str) -> Option<String> {
    let digits = word.strip_prefix('e')?;
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| word.to_uppercase())
}

/// The order id (`"O1"`) a word names, if any.
fn order_id(word: &str) -> Option<String> {
    let digits = word.strip_prefix('o')?;
    (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        .then(|| word.to_uppercase())
}

/// The number a word stands for: digits, an optional trailing `h`, or a
/// number word up to twelve.
fn number(word: &str) -> Option<u8> {
    const NAMES: [&str; 13] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        "eleven", "twelve",
    ];
    if let Some(i) = NAMES.iter().position(|n| *n == word) {
        return Some(i as u8);
    }
    let digits = word.strip_suffix('h').unwrap_or(word);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The first number among the words.
fn first_number(words: &[String]) -> Option<u8> {
    words.iter().find_map(|w| number(w))
}

/// The car kind a word names.
fn car_kind(word: &str) -> Option<CarKind> {
    match word {
        "boxcar" | "boxcars" | "box" | "boxes" => Some(CarKind::Boxcar),
        "tanker" | "tankers" | "tank" | "tanks" => Some(CarKind::Tanker),
        _ => None,
    }
}

/// The commodity a word names.
fn commodity(word: &str) -> Option<Commodity> {
    match word {
        "oranges" | "orange" => Some(Commodity::Oranges),
        "bananas" | "banana" => Some(Commodity::Bananas),
        "juice" => Some(Commodity::Juice),
        _ => None,
    }
}

/// Spoken name of a car kind.
fn kind_word(kind: CarKind) -> &'static str {
    match kind {
        CarKind::Boxcar => "boxcar",
        CarKind::Tanker => "tanker",
    }
}

/// Spoken name of a commodity.
fn commodity_word(c: Commodity) -> &'static str {
    match c {
        Commodity::Oranges => "oranges",
        Commodity::Bananas => "bananas",
        Commodity::Juice => "juice",
    }
}

/// Removes a `stop <n or station>` phrase from the words and returns it.
fn take_stop(words: &mut Vec<String>, vocab: &Vocabulary) -> Option<StopRef> {
    let at = words.iter().position(|w| w == "stop")?;
    let mut target = at + 1;
    if words.get(target).is_some_and(|w| w == "at") {
        target += 1;
    }
    let word = words.get(target)?;
    let stop = if let Some(n) = number(word) {
        StopRef::Index(n as usize)
    } else {
        StopRef::Station(vocab.station(word)?.to_string())
    };
    words.drain(at..=target);
    Some(stop)
}

/// Reads a departure condition from the words after `leave`.
fn parse_depart(words: &[String]) -> Result<Depart, String> {
    let usage = "Say when: \"leave now\", \"leave at 5\" or \"leave 1 after E2\".";
    if words
        .iter()
        .any(|w| matches!(w.as_str(), "now" | "immediately" | "once" | "asap"))
    {
        return Ok(Depart::Immediately);
    }
    if let Some(i) = words.iter().position(|w| w == "after") {
        let engine = words[i + 1..]
            .iter()
            .find_map(|w| engine_id(w))
            .ok_or_else(|| "Say which engine to wait for, e.g. \"leave 1 after E2\".".to_string())?;
        let plus_hours = first_number(&words[..i]).unwrap_or(0);
        return Ok(Depart::After { engine, plus_hours });
    }
    first_number(words)
        .map(|hour| Depart::At { hour })
        .ok_or_else(|| usage.to_string())
}

/// Reads an op from its verb and the words after it.
fn parse_op(verb: &str, words: &[String]) -> Result<Op, String> {
    let count = first_number(words).unwrap_or(1);
    let load = words.iter().find_map(|w| commodity(w));
    let filter = match load {
        Some(commodity) => LoadFilter::Loaded { commodity },
        None if words.iter().any(|w| matches!(w.as_str(), "empty" | "empties")) => {
            LoadFilter::Empty
        }
        None => LoadFilter::Any,
    };
    let kind = words.iter().find_map(|w| car_kind(w));
    match verb {
        "couple" | "uncouple" => {
            let kind = kind.ok_or_else(|| {
                format!("Say which cars to {verb}: a boxcar or a tanker, e.g. \"{verb} 1 boxcar\".")
            })?;
            Ok(if verb == "couple" {
                Op::Couple { kind, load: filter, count }
            } else {
                Op::Uncouple { kind, load: filter, count }
            })
        }
        "load" => load
            .map(|commodity| Op::Load { commodity, count })
            .ok_or_else(|| "Say what to load, e.g. \"load 2 oranges\".".to_string()),
        "process" => Ok(Op::Process { count }),
        _ => words
            .iter()
            .find_map(|w| order_id(w))
            .map(|order| Op::Deliver { order })
            .ok_or_else(|| "Say which order to deliver, e.g. \"deliver O1\".".to_string()),
    }
}

/// Reads one segment of a chat message.
fn parse_segment(raw: &str, vocab: &Vocabulary) -> Segment {
    let mut ws = words(raw);
    while ws.first().is_some_and(|w| FILLERS.contains(&w.as_str())) {
        ws.remove(0);
    }
    let Some(first) = ws.first().cloned() else {
        return Segment::Other;
    };
    if matches!(first.as_str(), "help" | "commands") {
        return Segment::Command(Command::Help);
    }
    let mentions_notes = ws.iter().any(|w| matches!(w.as_str(), "notes" | "note" | "plan"));
    if matches!(first.as_str(), "show" | "print" | "list" | "display" | "notes") && mentions_notes
    {
        return Segment::Command(Command::ShowNotes);
    }
    let lead_engine = engine_id(&first).is_some();
    let stop_lead = first == "stop" && ws.get(1).is_some_and(|w| number(w).is_some());
    if !(lead_engine || stop_lead || STARTERS.contains(&first.as_str())) {
        return Segment::Other;
    }
    // The engine is the first engine id that is not the target of "after".
    let engine = ws
        .iter()
        .enumerate()
        .find(|(i, w)| engine_id(w).is_some() && (*i == 0 || ws[i - 1] != "after"))
        .and_then(|(_, w)| engine_id(w));
    let stop = take_stop(&mut ws, vocab);
    let has = |verbs: &[&str]| ws.iter().position(|w| verbs.contains(&w.as_str()));
    let problem = |text: &str| Segment::Problem(text.to_string());
    let need_stop = |what: &str| format!("Which stop? Say e.g. \"E1 stop 1 {what}\".");

    if let Some(i) = has(CUT_VERBS) {
        return match first_number(&ws[i + 1..]) {
            Some(from) => Segment::Command(Command::Cut {
                engine,
                from: from as usize,
            }),
            None => problem("Say where to cut, e.g. \"E1 cut from 2\"."),
        };
    }
    if let Some(i) = has(REMOVE_VERBS) {
        let Some(op) = ws.iter().position(|w| matches!(w.as_str(), "op" | "ops")) else {
            return problem("Say which op to remove, e.g. \"E1 stop 1 remove op 0\".");
        };
        return match first_number(&ws[op.max(i) + 1..]) {
            Some(index) => Segment::Command(Command::RemoveOp {
                engine,
                stop: stop.unwrap_or(StopRef::Last),
                index: index as usize,
            }),
            None => problem("Say which op to remove, e.g. \"E1 stop 1 remove op 0\"."),
        };
    }
    if let Some(i) = has(LEAVE_VERBS) {
        let Some(stop) = stop else {
            return Segment::Problem(need_stop("leave at 5"));
        };
        return match parse_depart(&ws[i + 1..]) {
            Ok(depart) => Segment::Command(Command::Depart {
                engine,
                stop,
                depart,
            }),
            Err(text) => Segment::Problem(text),
        };
    }
    if let Some(i) = has(HEARD_VERBS) {
        let Some(stop) = stop else {
            return Segment::Problem(need_stop("heard 6"));
        };
        let after = &ws[i + 1..];
        let clear = after
            .iter()
            .any(|w| matches!(w.as_str(), "none" | "clear" | "nothing"));
        return match (clear, first_number(after)) {
            (true, _) => Segment::Command(Command::Heard {
                engine,
                stop,
                hour: None,
            }),
            (false, Some(h)) => Segment::Command(Command::Heard {
                engine,
                stop,
                hour: Some(h),
            }),
            (false, None) => problem("Say the hour, e.g. \"E1 stop 1 heard 6\", or \"heard none\"."),
        };
    }
    if let Some(i) = has(OP_VERBS) {
        return match parse_op(&ws[i].clone(), &ws[i + 1..]) {
            Ok(op) => Segment::Command(Command::AddOp {
                engine,
                stop: stop.unwrap_or(StopRef::Last),
                op,
            }),
            Err(text) => Segment::Problem(text),
        };
    }
    if let Some(i) = has(ROUTE_VERBS) {
        let stations: Vec<String> = ws[i + 1..]
            .iter()
            .filter_map(|w| vocab.station(w).map(str::to_string))
            .collect();
        if !stations.is_empty() {
            return Segment::Command(Command::Route { engine, stations });
        }
        if !lead_engine {
            return Segment::Other;
        }
        return Segment::Problem(format!(
            "Which station? I know {}.",
            vocab.ids().join(", ")
        ));
    }
    if lead_engine {
        Segment::Problem("I did not understand that. Type help for the commands.".to_string())
    } else {
        Segment::Other
    }
}

/// Reads one chat message into commands, problems and non-command text.
///
/// Segments are separated by `;` or a line break. A message without any
/// command yields empty `items` and the whole text in `rest`.
pub fn parse_message(text: &str, vocab: &Vocabulary) -> Parsed {
    let mut parsed = Parsed::default();
    let mut rest: Vec<&str> = Vec::new();
    for segment in text.split([';', '\n']) {
        match parse_segment(segment, vocab) {
            Segment::Other => rest.push(segment),
            Segment::Command(c) => parsed.items.push(Item::Command(c)),
            Segment::Problem(p) => parsed.items.push(Item::Problem(p)),
        }
    }
    parsed.rest = rest.join("\n");
    parsed
}

/// A command compiled against the partner's notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    /// The edits to propose, one per decision, in order. Empty for replies.
    pub edits: Vec<NoteEdit>,
    /// The text to send once the last edit is accepted (or at once when there
    /// are no edits).
    pub say: String,
}

/// Resolves the engine a command addresses against the notes.
fn resolve_engine(engine: &Option<String>, notes: &Plan) -> Result<String, String> {
    let known: Vec<&str> = notes.engines.keys().map(String::as_str).collect();
    match engine {
        Some(e) if notes.engines.contains_key(e) => Ok(e.clone()),
        Some(e) => Err(format!("I have no engine {e}. My engines: {}.", known.join(", "))),
        None if known.len() == 1 => Ok(known[0].to_string()),
        None => Err(format!("Which engine? Mine: {}.", known.join(", "))),
    }
}

/// Resolves a stop reference to an index in the engine's strip.
fn resolve_stop(stop: &StopRef, engine: &str, notes: &Plan) -> Result<usize, String> {
    let stops = &notes.engines[engine];
    match stop {
        StopRef::Index(i) => Ok(*i),
        StopRef::Last => Ok(stops.len() - 1),
        StopRef::Station(s) => stops
            .iter()
            .position(|st| &st.station == s)
            .ok_or_else(|| format!("{engine} has no stop at {s} in my notes.")),
    }
}

/// Spoken form of an op, in the words the parser reads back.
pub fn describe_op(op: &Op) -> String {
    let cars = |verb: &str, kind: CarKind, load: LoadFilter, count: u8| {
        let mut text = format!("{verb} {count} {}", kind_word(kind));
        match load {
            LoadFilter::Any => {}
            LoadFilter::Empty => text.push_str(" empty"),
            LoadFilter::Loaded { commodity } => {
                text.push(' ');
                text.push_str(commodity_word(commodity));
            }
        }
        text
    };
    match op {
        Op::Couple { kind, load, count } => cars("couple", *kind, *load, *count),
        Op::Uncouple { kind, load, count } => cars("uncouple", *kind, *load, *count),
        Op::Load { commodity, count } => format!("load {count} {}", commodity_word(*commodity)),
        Op::Process { count } => format!("process {count}"),
        Op::Deliver { order } => format!("deliver {order}"),
    }
}

/// Spoken form of a departure condition, in the words the parser reads back.
pub fn describe_depart(depart: &Depart) -> String {
    match depart {
        Depart::Immediately => "leave now".into(),
        Depart::At { hour } => format!("leave at {hour}"),
        Depart::After { engine, plus_hours } => format!("leave {plus_hours} after {engine}"),
    }
}

/// Spoken form of an edit, for rejection replies.
pub fn describe_edit(edit: &NoteEdit) -> String {
    match edit {
        NoteEdit::AppendStop { engine, station } => format!("{engine} to {station}"),
        NoteEdit::TruncateStops { engine, from } => format!("{engine} cut from {from}"),
        NoteEdit::SetDepart {
            engine,
            stop,
            depart,
        } => format!("{engine} stop {stop} {}", describe_depart(depart)),
        NoteEdit::InsertOp {
            engine, stop, op, ..
        } => format!("{engine} stop {stop} {}", describe_op(op)),
        NoteEdit::RemoveOp {
            engine,
            stop,
            index,
        } => format!("{engine} stop {stop} remove op {index}"),
        NoteEdit::SetHeardArrival { engine, stop, hour } => match hour {
            Some(h) => format!("{engine} stop {stop} heard {h}"),
            None => format!("{engine} stop {stop} heard none"),
        },
    }
}

/// Lists the notes compactly, one line per engine, in the parser's words.
pub fn describe_notes(notes: &Plan) -> String {
    let mut lines = vec!["My notes:".to_string()];
    for (engine, stops) in &notes.engines {
        let stops: Vec<String> = stops
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut head = format!("{i} {} {}", s.station, describe_depart(&s.depart));
                if let Some(h) = s.heard_arrival {
                    head.push_str(&format!(", heard {h}"));
                }
                if s.ops.is_empty() {
                    format!("[{head}]")
                } else {
                    let ops: Vec<String> = s.ops.iter().map(describe_op).collect();
                    format!("[{head} | {}]", ops.join(", "))
                }
            })
            .collect();
        lines.push(format!("Strip {engine}: {}", stops.join(" > ")));
    }
    lines.join("\n")
}

/// A reminder that a field is not used by the partner's role in a run.
fn unused_field(edit: &NoteEdit, role: Seat) -> Option<&'static str> {
    match (edit, role) {
        (NoteEdit::SetHeardArrival { .. }, _) => {
            Some("A heard arrival is only a jotted annotation and never affects a run.")
        }
        (
            NoteEdit::AppendStop { .. }
            | NoteEdit::TruncateStops { .. }
            | NoteEdit::SetDepart { .. },
            Seat::Freight,
        ) => Some("As Freight, my stations and departures are not used in a run; Routing's are."),
        (NoteEdit::InsertOp { .. } | NoteEdit::RemoveOp { .. }, Seat::Routing) => {
            Some("As Routing, my ops are not used in a run; Freight's are.")
        }
        _ => None,
    }
}

/// Compiles a command against the partner's own notes.
///
/// Returns the edits and the confirmation to send, or the reply that explains
/// why the command cannot be carried out. `role` selects the reminder that a
/// field is not used by the partner's role in a run.
pub fn compile(command: &Command, notes: &Plan, role: Seat) -> Result<Compiled, String> {
    let (edits, say) = match command {
        Command::Help => {
            return Ok(Compiled {
                edits: vec![],
                say: HELP.to_string(),
            })
        }
        Command::ShowNotes => {
            return Ok(Compiled {
                edits: vec![],
                say: describe_notes(notes),
            })
        }
        Command::Route { engine, stations } => {
            let engine = resolve_engine(engine, notes)?;
            let edits = stations
                .iter()
                .map(|s| NoteEdit::AppendStop {
                    engine: engine.clone(),
                    station: s.clone(),
                })
                .collect();
            let say = format!("Done: {engine} now goes on to {}.", stations.join(", "));
            (edits, say)
        }
        Command::Cut { engine, from } => {
            let engine = resolve_engine(engine, notes)?;
            let edits = vec![NoteEdit::TruncateStops {
                engine: engine.clone(),
                from: *from,
            }];
            let say = format!("Done: cut {engine} back to stop {}.", from.saturating_sub(1));
            (edits, say)
        }
        Command::Depart {
            engine,
            stop,
            depart,
        } => {
            let engine = resolve_engine(engine, notes)?;
            let stop = resolve_stop(stop, &engine, notes)?;
            let say = format!("Done: {engine} stop {stop}, {}.", describe_depart(depart));
            let edits = vec![NoteEdit::SetDepart {
                engine: engine.clone(),
                stop,
                depart: depart.clone(),
            }];
            (edits, say)
        }
        Command::AddOp { engine, stop, op } => {
            let engine = resolve_engine(engine, notes)?;
            let stop = resolve_stop(stop, &engine, notes)?;
            let index = notes.engines[&engine]
                .get(stop)
                .map_or(0, |s| s.ops.len());
            let say = format!("Done: {engine} stop {stop}, added {}.", describe_op(op));
            let edits = vec![NoteEdit::InsertOp {
                engine: engine.clone(),
                stop,
                index,
                op: op.clone(),
            }];
            (edits, say)
        }
        Command::RemoveOp {
            engine,
            stop,
            index,
        } => {
            let engine = resolve_engine(engine, notes)?;
            let stop = resolve_stop(stop, &engine, notes)?;
            let say = format!("Done: {engine} stop {stop}, removed op {index}.");
            let edits = vec![NoteEdit::RemoveOp {
                engine: engine.clone(),
                stop,
                index: *index,
            }];
            (edits, say)
        }
        Command::Heard { engine, stop, hour } => {
            let engine = resolve_engine(engine, notes)?;
            let stop = resolve_stop(stop, &engine, notes)?;
            let say = match hour {
                Some(h) => format!("Done: {engine} stop {stop}, heard arrival {h}."),
                None => format!("Done: {engine} stop {stop}, heard arrival cleared."),
            };
            let edits = vec![NoteEdit::SetHeardArrival {
                engine: engine.clone(),
                stop,
                hour: *hour,
            }];
            (edits, say)
        }
    };
    let say = match edits.first().and_then(|e| unused_field(e, role)) {
        Some(reminder) => format!("{say} {reminder}"),
        None => say,
    };
    Ok(Compiled { edits, say })
}

/// A rejection code in plain words, as a clause that follows "because".
pub fn reject_words(code: &str) -> String {
    match code {
        "wrong_phase" => "notes can only be edited while planning",
        "unknown_engine" => "there is no such engine",
        "unknown_station" => "there is no such station",
        "unknown_order" => "there is no such order",
        "not_adjacent" => "those stations are not directly connected",
        "index_out_of_range" => "that stop or op number does not exist",
        "fixed_stop" => "stop 0 is the starting station and cannot be cut or changed",
        "too_many_stops" => "an engine can have at most 12 stops",
        "too_many_ops" => "a stop can have at most 6 ops",
        "invalid_count" => "a car count must be between 1 and 9",
        "incompatible_load" => "that kind of car cannot carry that commodity",
        "invalid_hour" => "that hour is outside the day",
        "no_change" => "my notes already say that",
        other => return format!("the game said {other}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Stop;

    /// The tutorial map's stations.
    fn vocab() -> Vocabulary {
        Vocabulary::new([
            ("avon", "Avon"),
            ("bath", "Bath"),
            ("corning", "Corning"),
            ("dansville", "Dansville"),
            ("elmira", "Elmira"),
        ])
    }

    /// Parses a message that must hold exactly one command.
    fn one(text: &str) -> Command {
        let parsed = parse_message(text, &vocab());
        match parsed.items.as_slice() {
            [Item::Command(c)] => c.clone(),
            other => panic!("{text:?} gave {other:?}"),
        }
    }

    /// Parses a message that must hold exactly one problem.
    fn problem(text: &str) -> String {
        let parsed = parse_message(text, &vocab());
        match parsed.items.as_slice() {
            [Item::Problem(p)] => p.clone(),
            other => panic!("{text:?} gave {other:?}"),
        }
    }

    fn e1() -> Option<String> {
        Some("E1".into())
    }

    #[test]
    fn routes_append_stations_in_order() {
        let want = Command::Route {
            engine: e1(),
            stations: vec!["dansville".into()],
        };
        assert_eq!(one("E1 to Dansville"), want);
        assert_eq!(one("please send e1 to dansville!"), want);
        assert_eq!(
            one("E1 go Dansville, Corning and Bath"),
            Command::Route {
                engine: e1(),
                stations: vec!["dansville".into(), "corning".into(), "bath".into()]
            }
        );
        assert!(matches!(one("go elmira"), Command::Route { engine: None, .. }));
    }

    #[test]
    fn cut_reads_the_first_number() {
        assert_eq!(one("E1 cut from 2"), Command::Cut { engine: e1(), from: 2 });
        assert_eq!(one("e1 truncate at three"), Command::Cut { engine: e1(), from: 3 });
        assert!(problem("E1 cut it").contains("where to cut"));
    }

    #[test]
    fn departures() {
        let at = |h| Command::Depart {
            engine: e1(),
            stop: StopRef::Index(2),
            depart: Depart::At { hour: h },
        };
        assert_eq!(one("E1 stop 2 leave at 5"), at(5));
        assert_eq!(one("E1 stop 2 should leave at 5h please"), at(5));
        assert_eq!(
            one("E1 stop 2 leave now"),
            Command::Depart {
                engine: e1(),
                stop: StopRef::Index(2),
                depart: Depart::Immediately
            }
        );
        assert_eq!(
            one("E1 stop 2 leave 1 after E2"),
            Command::Depart {
                engine: e1(),
                stop: StopRef::Index(2),
                depart: Depart::After {
                    engine: "E2".into(),
                    plus_hours: 1
                }
            }
        );
        // The engine named after "after" is not the addressed engine.
        assert!(matches!(
            one("stop 1 leave after E2"),
            Command::Depart { engine: None, depart: Depart::After { plus_hours: 0, .. }, .. }
        ));
        assert!(problem("E1 leave at 5").contains("Which stop"));
        assert!(problem("E1 stop 1 leave").contains("Say when"));
    }

    #[test]
    fn ops_go_to_the_named_or_last_stop() {
        assert_eq!(
            one("E1 stop 0 couple 2 boxcar"),
            Command::AddOp {
                engine: e1(),
                stop: StopRef::Index(0),
                op: Op::Couple { kind: CarKind::Boxcar, load: LoadFilter::Any, count: 2 }
            }
        );
        assert_eq!(
            one("uncouple 1 boxcar oranges"),
            Command::AddOp {
                engine: None,
                stop: StopRef::Last,
                op: Op::Uncouple { kind: CarKind::Boxcar, load: LoadFilter::Loaded { commodity: Commodity::Oranges }, count: 1 }
            }
        );
        assert_eq!(
            one("E1 stop 0 couple 1 boxcar empty"),
            Command::AddOp {
                engine: e1(),
                stop: StopRef::Index(0),
                op: Op::Couple { kind: CarKind::Boxcar, load: LoadFilter::Empty, count: 1 }
            }
        );
        assert_eq!(
            one("Load two oranges"),
            Command::AddOp {
                engine: None,
                stop: StopRef::Last,
                op: Op::Load { commodity: Commodity::Oranges, count: 2 }
            }
        );
        assert!(matches!(
            one("process 1"),
            Command::AddOp { op: Op::Process { count: 1 }, .. }
        ));
        assert_eq!(
            one("E1 stop dansville deliver o1"),
            Command::AddOp {
                engine: e1(),
                stop: StopRef::Station("dansville".into()),
                op: Op::Deliver { order: "O1".into() }
            }
        );
        assert!(problem("couple 1").contains("boxcar or a tanker"));
        assert!(problem("load some").contains("what to load"));
        assert!(problem("deliver it").contains("which order"));
    }

    #[test]
    fn remove_and_heard() {
        assert_eq!(
            one("E1 stop 1 remove op 0"),
            Command::RemoveOp { engine: e1(), stop: StopRef::Index(1), index: 0 }
        );
        assert!(problem("E1 stop 1 remove it").contains("which op"));
        assert_eq!(
            one("E1 stop 1 heard 6"),
            Command::Heard { engine: e1(), stop: StopRef::Index(1), hour: Some(6) }
        );
        assert_eq!(
            one("E1 stop 1 heard none"),
            Command::Heard { engine: e1(), stop: StopRef::Index(1), hour: None }
        );
    }

    #[test]
    fn help_and_show() {
        assert_eq!(one("help"), Command::Help);
        assert_eq!(one("Please show notes"), Command::ShowNotes);
        assert_eq!(one("show me my plan"), Command::ShowNotes);
    }

    /// Chat that is not a command goes to the cues untouched.
    #[test]
    fn other_chat_is_left_for_the_cues() {
        for text in [
            "ready",
            "ok, I'm ready",
            "let's go",
            "go ahead",
            "wait",
            "stop",
            "what do you know about the orders?",
            "the oranges are at Corning",
        ] {
            let parsed = parse_message(text, &vocab());
            assert!(parsed.items.is_empty(), "{text:?}: {:?}", parsed.items);
            assert_eq!(parsed.rest, text);
        }
    }

    #[test]
    fn segments_split_on_semicolons_and_keep_the_rest() {
        let parsed = parse_message("E1 to dansville; E1 stop 0 leave at 1\nready", &vocab());
        assert_eq!(parsed.items.len(), 2);
        assert_eq!(parsed.rest.trim(), "ready");
    }

    #[test]
    fn unusable_engine_commands_get_a_reply() {
        assert!(problem("E1 to nowhere").contains("Which station"));
        assert!(problem("E1 is cool").contains("did not understand"));
    }

    /// A one-engine strip with two stops, for compile tests.
    fn notes() -> Plan {
        let mut plan = Plan::initial([("E1", "corning")]);
        plan.engines
            .get_mut("E1")
            .unwrap()
            .push(Stop::at("dansville"));
        plan
    }

    #[test]
    fn compile_resolves_defaults() {
        let c = compile(&one("deliver O1"), &notes(), Seat::Freight).unwrap();
        assert_eq!(
            c.edits,
            vec![NoteEdit::InsertOp {
                engine: "E1".into(),
                stop: 1,
                index: 0,
                op: Op::Deliver { order: "O1".into() }
            }]
        );
        let c = compile(&one("E1 stop corning leave at 1"), &notes(), Seat::Routing).unwrap();
        assert!(matches!(c.edits[0], NoteEdit::SetDepart { stop: 0, .. }));
        assert!(!c.say.contains("not used"));
    }

    #[test]
    fn compile_appends_one_edit_per_station() {
        let c = compile(&one("E1 go bath corning"), &notes(), Seat::Routing).unwrap();
        assert_eq!(c.edits.len(), 2);
    }

    #[test]
    fn compile_reminds_about_unused_fields() {
        let c = compile(&one("E1 stop 1 deliver O1"), &notes(), Seat::Routing).unwrap();
        assert!(c.say.contains("my ops are not used"));
        let c = compile(&one("E1 stop 1 leave at 3"), &notes(), Seat::Freight).unwrap();
        assert!(c.say.contains("departures are not used"));
        let c = compile(&one("E1 stop 1 heard 3"), &notes(), Seat::Routing).unwrap();
        assert!(c.say.contains("annotation"));
    }

    #[test]
    fn compile_explains_engine_and_stop_problems() {
        let err = compile(&one("E7 to bath"), &notes(), Seat::Routing).unwrap_err();
        assert!(err.contains("no engine E7"));
        let err = compile(&one("E1 stop avon leave now"), &notes(), Seat::Routing).unwrap_err();
        assert!(err.contains("no stop at avon"));
        let mut two = notes();
        two.engines.insert("E2".into(), vec![Stop::at("avon")]);
        let err = compile(&one("deliver O1"), &two, Seat::Freight).unwrap_err();
        assert!(err.contains("Which engine"));
    }

    /// `show notes` lists the strips in words the parser reads back.
    #[test]
    fn notes_listing_is_compact() {
        let mut plan = notes();
        plan.engines.get_mut("E1").unwrap()[1].ops.push(Op::Deliver { order: "O1".into() });
        plan.engines.get_mut("E1").unwrap()[0].depart = Depart::At { hour: 1 };
        assert_eq!(
            describe_notes(&plan),
            "My notes:\nStrip E1: [0 corning leave at 1] > [1 dansville leave now | deliver O1]"
        );
    }

    #[test]
    fn rejection_codes_have_words() {
        assert!(reject_words("not_adjacent").contains("directly connected"));
        assert_eq!(reject_words("odd"), "the game said odd");
    }
}
