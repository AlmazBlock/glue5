//! Generates the natives' table (`natives.rs`) and their Luau definitions
//! (`natives.d.luau`) from what game.wasm says about each native (glue5-game's
//! build script, `DEP_GLUE5_GAME_NATIVES`) and, when present, the natives
//! database (`third_party/nativedb/natives.json`, from `mise run setup`):
//! names of parameters, documentation, the handle types.
//!
//! The binary decides what can be called and how (handler, slots); the
//! database adds what the binary does not say. Without the database the
//! natives with a typed command function are still there, with parameters
//! named `p0`, `p1`...

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

use anyhow::{Context, Result};
use serde_json::Value;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

/// What the generated code calls a value (glue5-mods' `Kind`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Int,
    Float,
    Bool,
    Hash,
    Str,
    Vec3,
    Any,
    Handle(Handle),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(clippy::enum_variant_names)] // ScrHandle: the database's name
enum Handle {
    Entity,
    Ped,
    Vehicle,
    Object,
    Player,
    Cam,
    Blip,
    Pickup,
    Interior,
    ScrHandle,
    FireId,
}

const HANDLES: &[(&str, Handle, Option<&str>)] = &[
    ("Entity", Handle::Entity, None),
    ("Ped", Handle::Ped, Some("Entity")),
    ("Vehicle", Handle::Vehicle, Some("Entity")),
    ("Object", Handle::Object, Some("Entity")),
    ("Player", Handle::Player, None),
    ("Cam", Handle::Cam, None),
    ("Blip", Handle::Blip, None),
    ("Pickup", Handle::Pickup, None),
    ("Interior", Handle::Interior, None),
    ("ScrHandle", Handle::ScrHandle, None),
    ("FireId", Handle::FireId, None),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dir {
    In,
    Out,
    InOut,
}

#[derive(Clone, Debug)]
struct Param {
    name: String,
    kind: Kind,
    dir: Dir,
}

impl Param {
    fn slots(&self) -> usize {
        if self.kind == Kind::Vec3 && self.dir == Dir::In { 3 } else { 1 }
    }
}

struct Native {
    name: String,
    hash: u64,
    handler: u64,
    namespace: String,
    lua: String,
    params: Vec<Param>,
    ret: Option<Kind>,
    doc: String,
}

/// One line of glue5-game's table.
struct Binary {
    name: String,
    hash: u64,
    handler: u64,
    unused: bool,
    group: String,
    command: Option<Vec<String>>,
    /// The typed command's wasm result: "", "i32", "f32", "i64"; "sret"
    /// for a vector returned through a pointer.
    result: Option<String>,
    signature: u64,
}

/// Natives the mods may not call: the script thread's own control (a mod's
/// thread only waits through `wait`, and ends through `mod`).
const RESERVED: &[&str] = &["WAIT", "TERMINATE_THIS_THREAD"];

fn run() -> Result<()> {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let root = manifest.parent().and_then(Path::parent).context("workspace root")?.to_path_buf();
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let table = PathBuf::from(env::var("DEP_GLUE5_GAME_NATIVES").context("glue5-game's natives table")?);
    let db_path = root.join("third_party/nativedb/natives.json");
    println!("cargo:rerun-if-changed={}", table.display());
    println!("cargo:rerun-if-changed={}", db_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let binary = parse_table(&fs::read_to_string(&table)?)?;
    let db: Option<Value> = match fs::read(&db_path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).context("parsing natives.json")?),
        Err(_) => {
            println!(
                "cargo:warning=no {} (`mise run setup`): natives without documentation or names of parameters",
                db_path.display()
            );
            None
        }
    };
    // The database's entries by name, with their namespace.
    let mut by_name: HashMap<String, (String, &Value)> = HashMap::new();
    if let Some(Value::Object(spaces)) = &db {
        for (space, entries) in spaces {
            if let Value::Object(entries) = entries {
                for entry in entries.values() {
                    if let Some(name) = entry["name"].as_str() {
                        by_name.insert(name.to_string(), (space.clone(), entry));
                    }
                }
            }
        }
    }
    // Each of the binary's groups takes the namespace most of its natives
    // have in the database (camera -> CAM), so the names match the docs.
    let mut votes: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    for b in &binary {
        if let Some((space, _)) = by_name.get(&b.name) {
            *votes.entry(&b.group).or_default().entry(space).or_default() += 1;
        }
    }
    let namespace_of = |group: &str| -> String {
        votes
            .get(group)
            .and_then(|v| v.iter().max_by_key(|&(name, n)| (*n, std::cmp::Reverse(*name))).map(|(s, _)| s.to_ascii_lowercase()))
            .unwrap_or_else(|| group.to_string())
    };

    let mut natives = Vec::new();
    let mut skipped = BTreeMap::<&str, usize>::new();
    let mut reserved = Vec::new();
    for b in &binary {
        if RESERVED.contains(&b.name.as_str()) {
            reserved.push((b.name.clone(), b.handler));
            *skipped.entry("reserved").or_default() += 1;
            continue;
        }
        let entry = by_name.get(&b.name).map(|(_, e)| *e);
        let from_db = entry.map(|e| db_params(e, &b.name)).transpose()?;
        let from_binary = b.command.as_ref().map(|c| binary_params(c, &b.name));
        let params = match (from_db, from_binary) {
            (Some(Some(db)), Some(Some(bin))) => merge(db, &bin),
            (Some(Some(db)), _) => db,
            (_, Some(Some(bin))) => bin,
            _ => {
                *skipped.entry(if b.unused { "unused, untyped" } else { "untyped" }).or_default() += 1;
                continue;
            }
        };
        let ret = match entry.and_then(|e| e["return_type"].as_str()) {
            Some(t) => match value_kind(t) {
                Some(k) => Some(k),
                None if t == "void" => None,
                None => {
                    *skipped.entry("return type").or_default() += 1;
                    continue;
                }
            },
            None => match binary_return(b) {
                Some(r) => r,
                None => {
                    *skipped.entry("return type").or_default() += 1;
                    continue;
                }
            },
        };
        let doc = entry.and_then(|e| e["comment"].as_str()).unwrap_or("").to_string();
        natives.push(Native {
            name: b.name.clone(),
            hash: b.hash,
            handler: b.handler,
            namespace: namespace_of(&b.group),
            lua: camel(&b.name),
            params,
            ret,
            doc,
        });
    }
    natives.sort_by(|a, b| (&a.namespace, &a.lua).cmp(&(&b.namespace, &b.lua)));
    let skipped_text = skipped.iter().map(|(why, n)| format!("{n} {why}")).collect::<Vec<_>>().join(", ");
    println!("cargo:rustc-env=GLUE5_MODS_NATIVES_SUMMARY={} natives; not exposed: {skipped_text}", natives.len());

    fs::write(out.join("natives.rs"), rust_table(&natives, &reserved))?;
    fs::write(out.join("natives.d.luau"), definitions(&natives))?;
    Ok(())
}

fn parse_table(text: &str) -> Result<Vec<Binary>> {
    text.lines()
        .map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            anyhow::ensure!(f.len() == 8, "bad natives line {line:?}");
            let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16);
            let command = (f[5] != "-").then(|| {
                if f[5] == "void" { Vec::new() } else { f[5].split(", ").map(str::to_string).collect() }
            });
            let result = (f[6] != "-").then(|| {
                let (params, result) = f[6].split_once("->").unwrap_or(("", ""));
                let wasm_params = params.split(',').filter(|p| !p.is_empty()).count();
                let typed = command.as_ref().map_or(0, Vec::len);
                if result.is_empty() && wasm_params == typed + 1 { "sret".to_string() } else { result.to_string() }
            });
            Ok(Binary {
                name: f[0].to_string(),
                hash: hex(f[1])?,
                handler: f[2].parse()?,
                unused: f[3] == "1",
                group: f[4].to_string(),
                command,
                result,
                signature: hex(f[7])?,
            })
        })
        .collect()
}

fn handle(name: &str) -> Option<Handle> {
    HANDLES.iter().find(|(n, ..)| *n == name).map(|&(_, h, _)| h)
}

/// A value type of the database.
fn value_kind(t: &str) -> Option<Kind> {
    Some(match t {
        "int" => Kind::Int,
        "float" => Kind::Float,
        "BOOL" => Kind::Bool,
        "Hash" => Kind::Hash,
        "const char*" => Kind::Str,
        "Vector3" => Kind::Vec3,
        "Any" => Kind::Any,
        other => Kind::Handle(handle(other)?),
    })
}

/// Pointers to a value: outputs of getters and predicates, else the value
/// goes in and comes back (`DELETE_ENTITY(Entity*)`).
fn pointer_dir(native: &str) -> Dir {
    let getter = ["GET_", "IS_", "HAS_", "_GET_", "DOES_", "FIND_", "CAN_"].iter().any(|p| native.starts_with(p));
    if getter { Dir::Out } else { Dir::InOut }
}

/// The database's parameters, `None` when one has no type the mods can
/// pass (pointers to structures, buffers).
fn db_params(entry: &Value, native: &str) -> Result<Option<Vec<Param>>> {
    let mut params = Vec::new();
    for p in entry["params"].as_array().context("params")? {
        let t = p["type"].as_str().context("type")?;
        let name = p["name"].as_str().unwrap_or("");
        let (kind, dir) = if t == "const char*" {
            (Kind::Str, Dir::In)
        } else if let Some(inner) = t.strip_suffix('*') {
            match value_kind(inner) {
                Some(k) if !matches!(k, Kind::Any | Kind::Str) => (k, pointer_dir(native)),
                _ => return Ok(None),
            }
        } else {
            match value_kind(t) {
                Some(k) => (k, Dir::In),
                None => return Ok(None),
            }
        };
        params.push(Param { name: param_name(name), kind, dir });
    }
    Ok(Some(params))
}

/// The typed command's C++ parameters.
fn binary_params(command: &[String], native: &str) -> Option<Vec<Param>> {
    command
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (kind, dir) = match t.as_str() {
                "int" | "unsigned int" => (Kind::Int, Dir::In),
                "bool" => (Kind::Bool, Dir::In),
                "float" => (Kind::Float, Dir::In),
                "char const*" => (Kind::Str, Dir::In),
                "rage::scrVector const&" | "rage::Vector3 const&" => (Kind::Vec3, Dir::In),
                "int&" => (Kind::Int, pointer_dir(native)),
                "float&" => (Kind::Float, pointer_dir(native)),
                "bool&" => (Kind::Bool, pointer_dir(native)),
                "rage::Vector3&" | "rage::scrVector&" => (Kind::Vec3, pointer_dir(native)),
                _ => return None,
            };
            Some(Param { name: format!("p{i}"), kind, dir })
        })
        .collect()
}

/// The database's parameters checked against the binary's: the same slots,
/// with the binary's vectors where the database has three floats.
fn merge(db: Vec<Param>, bin: &[Param]) -> Vec<Param> {
    let slots = |v: &[Param]| v.iter().map(Param::slots).sum::<usize>();
    if slots(&db) != slots(bin) {
        return bin.to_vec();
    }
    let mut out = Vec::new();
    let mut i = 0;
    for b in bin {
        if b.kind == Kind::Vec3 && b.dir == Dir::In && db.get(i).is_some_and(|p| p.kind == Kind::Float) {
            let name = vector_name(&db[i].name);
            out.push(Param { name, kind: Kind::Vec3, dir: Dir::In });
            i += 3;
        } else if let Some(p) = db.get(i) {
            out.push(p.clone());
            i += 1;
        }
    }
    out
}

/// What the binary says about the result, when the database has nothing.
fn binary_return(b: &Binary) -> Option<Option<Kind>> {
    match b.result.as_deref() {
        Some("") => Some(None),
        Some("sret") => Some(Some(Kind::Vec3)),
        Some("f32") => Some(Some(Kind::Float)),
        Some("i64") => Some(Some(Kind::Str)),
        Some("i32") => Some(Some(if (b.signature >> 1) & 7 == 2 { Kind::Bool } else { Kind::Int })),
        _ => None,
    }
}

/// `CREATE_VEHICLE` -> `createVehicle`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    for (i, word) in name.split('_').filter(|w| !w.is_empty()).enumerate() {
        let lower = word.to_ascii_lowercase();
        if i == 0 {
            out.push_str(&lower);
        } else {
            let mut chars = lower.chars();
            if let Some(c) = chars.next() {
                out.push(c.to_ascii_uppercase());
                out.extend(chars);
            }
        }
    }
    out
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not",
    "or", "repeat", "return", "then", "true", "until", "while", "continue", "type", "export",
];

/// A database parameter's name in Luau's style: `bScriptHostVeh` ->
/// `scriptHostVeh`, snake case to camel case, no keywords.
fn param_name(name: &str) -> String {
    let mut n = name.to_string();
    let bytes = n.as_bytes();
    if bytes.len() > 1 && bytes[0] == b'b' && bytes[1].is_ascii_uppercase() {
        n = format!("{}{}", n[1..2].to_ascii_lowercase(), &n[2..]);
    }
    if n.contains('_') {
        let mut c = camel(&n);
        if c.is_empty() {
            c = "value".into();
        }
        n = c;
    }
    if n.is_empty() || !n.chars().next().unwrap().is_ascii_alphabetic() {
        n = format!("p{n}");
    }
    if KEYWORDS.contains(&n.as_str()) {
        n.push('_');
    }
    n
}

/// `x` -> `coords`, `posX` -> `pos`, `x1` -> `coords1`.
fn vector_name(x: &str) -> String {
    let lower = x.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix('x')
        && rest.chars().all(|c| c.is_ascii_digit())
    {
        return format!("coords{rest}");
    }
    let stem = x.strip_suffix('X').or_else(|| x.strip_suffix('x')).unwrap_or(x);
    if stem.is_empty() { "coords".into() } else { stem.to_string() }
}

fn rust_kind(k: Kind) -> String {
    match k {
        Kind::Handle(h) => format!("K::Handle(H::{h:?})"),
        other => format!("K::{other:?}"),
    }
}

fn rust_table(natives: &[Native], reserved: &[(String, u64)]) -> String {
    let mut spaces: Vec<&str> = natives.iter().map(|n| n.namespace.as_str()).collect();
    spaces.dedup();
    let mut s = String::new();
    s.push_str("// Generated by glue5-mods' build script from game.wasm and nativedb.\n");
    let _ = writeln!(s, "pub const NAMESPACES: &[&str] = &{spaces:?};");
    s.push_str("pub static NATIVES: &[Native] = &[\n");
    for n in natives {
        let ns = spaces.iter().position(|&x| x == n.namespace).unwrap();
        let params: Vec<String> = n
            .params
            .iter()
            .map(|p| format!("P {{ kind: {}, dir: D::{:?} }}", rust_kind(p.kind), p.dir))
            .collect();
        let ret = n.ret.map_or("None".to_string(), |k| format!("Some({})", rust_kind(k)));
        let _ = writeln!(
            s,
            "    Native {{ ns: {ns}, lua: {:?}, name: {:?}, hash: {:#x}, handler: {}, params: &[{}], ret: {ret} }},",
            n.lua,
            n.name,
            n.hash,
            n.handler,
            params.join(", ")
        );
    }
    s.push_str("];\n");
    let _ = writeln!(s, "pub const RESERVED: &[(&str, u64)] = &{reserved:?};");
    s
}

fn luau_type(k: Kind, input: bool) -> String {
    match k {
        Kind::Int | Kind::Float => "number".into(),
        Kind::Bool => "boolean".into(),
        Kind::Hash if input => "Hash | string".into(),
        Kind::Hash => "Hash".into(),
        Kind::Str => "string".into(),
        Kind::Vec3 => "vector".into(),
        Kind::Any => "any".into(),
        Kind::Handle(h) => format!("{h:?}"),
    }
}

/// Handles the natives return: 0 (none) becomes nil, so they are optional
/// (but for players: 0 is the local one).
fn luau_result(k: Kind) -> String {
    match k {
        Kind::Handle(Handle::Player) => "Player".into(),
        Kind::Handle(h) => format!("{h:?}?"),
        other => luau_type(other, false),
    }
}

fn definitions(natives: &[Native]) -> String {
    let mut s = String::new();
    s.push_str("--!strict\n-- The game's natives, generated from game.wasm and nativedb by glue5-mods. Do not edit.\n\n");
    for (name, _, base) in HANDLES {
        match base {
            Some(b) => {
                let _ = writeln!(s, "declare class {name} extends {b} end");
            }
            None => {
                let _ = writeln!(s, "declare class {name} end");
            }
        }
    }
    s.push_str("type Hash = number\n\n");
    let mut i = 0;
    while i < natives.len() {
        let ns = &natives[i].namespace;
        let _ = writeln!(s, "declare {ns}: {{");
        while i < natives.len() && &natives[i].namespace == ns {
            let n = &natives[i];
            let doc = n.doc.lines().map(str::trim).filter(|l| !l.is_empty()).take(6).collect::<Vec<_>>();
            for line in &doc {
                let _ = writeln!(s, "\t--- {line}");
            }
            let _ = writeln!(s, "\t--- `{}`", n.name);
            let inputs: Vec<String> = n
                .params
                .iter()
                .filter(|p| p.dir != Dir::Out)
                .map(|p| format!("{}: {}", p.name, luau_type(p.kind, true)))
                .collect();
            let mut outputs: Vec<String> = n.ret.into_iter().map(luau_result).collect();
            outputs.extend(n.params.iter().filter(|p| p.dir != Dir::In).map(|p| luau_result(p.kind)));
            let result = match outputs.len() {
                0 => "()".to_string(),
                1 if !outputs[0].contains('|') => outputs[0].clone(),
                _ => format!("({})", outputs.join(", ")),
            };
            let _ = writeln!(s, "\t{}: ({}) -> {},", n.lua, inputs.join(", "), result);
            i += 1;
        }
        s.push_str("}\n\n");
    }
    s
}
