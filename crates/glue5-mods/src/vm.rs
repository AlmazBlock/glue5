//! A mod's Luau VM: sandboxed, with the natives as namespace tables, `wait`
//! and `require` (the mod's own files); its main chunk runs as a coroutine
//! the engine resumes once per wait. The REPL's VM (`eval`) is the same
//! with a loop that runs the snippets it is given.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Table, Thread, Value, VmState};

use crate::{EvalResult, natives};

/// How long a mod may run before it yields to the game for this frame.
const SLICE: Duration = Duration::from_millis(2);
/// How long a mod may run without calling `wait` before it is stopped.
const RUNAWAY: Duration = Duration::from_secs(2);
/// Memory a mod's VM may use.
const MEMORY: usize = 64 << 20;

/// Snippets for the REPL: code, and where its result goes.
pub type Jobs = Rc<RefCell<VecDeque<(String, Sender<EvalResult>)>>>;

pub struct Vm {
    lua: Lua,
    main: Thread,
    deadline: Rc<Cell<Instant>>,
    /// Time run since the mod last called `wait`.
    busy: Duration,
}

/// What a slice of a mod's run came to.
pub enum Step {
    /// It waits this many milliseconds (0: the next frame).
    Wait(u32),
    /// It ran out of time in this frame and goes on in the next.
    Preempted,
    Finished,
    Failed(String),
}

impl Vm {
    /// Loads mod `name`'s `main.luau` from `dir`; `log` gets what it prints.
    pub fn new(name: &str, dir: &Path, log: Rc<dyn Fn(&str)>) -> mlua::Result<Vm> {
        let (lua, deadline) = base(name, log)?;
        lua.globals().set("require", require(&lua, name, dir)?)?;
        // The mod's code starts once the player is in the game (not on
        // the loading screens, where the player's ped is a stand-in).
        let ready: Function = lua
            .load(
                "local yield = coroutine.yield\n\
                 return function(main) return function(...)\n\
                 while dlc.getIsLoadingScreenActive() or not camera.isScreenFadedIn() \
                 or not player.isPlayerPlaying(player.playerId()) do yield(0) end\n\
                 return main(...)\n\
                 end end",
            )
            .set_name("=ready")
            .eval()?;
        lua.sandbox(true)?;
        let path = dir.join("main.luau");
        let source = std::fs::read_to_string(&path)
            .map_err(|e| mlua::Error::runtime(format!("reading {}: {e}", path.display())))?;
        let chunk = lua.load(source).set_name(format!("@{name}/main.luau")).into_function()?;
        let main = lua.create_thread(ready.call::<Function>(chunk)?)?;
        Ok(Vm { lua, main, deadline, busy: Duration::ZERO })
    }

    /// The REPL: runs `jobs`' snippets, as expressions when they are.
    pub fn repl(jobs: Jobs, log: Rc<dyn Fn(&str)>) -> mlua::Result<Vm> {
        let (lua, deadline) = base("eval", log)?;
        let waiting: Rc<RefCell<HashMap<i64, Sender<EvalResult>>>> = Rc::default();
        let next = Cell::new(0i64);
        let pending = waiting.clone();
        let take = lua.create_function(move |lua, ()| {
            let Some((code, reply)) = jobs.borrow_mut().pop_front() else { return Ok(MultiValue::new()) };
            let id = next.get() + 1;
            next.set(id);
            let compiled = lua
                .load(format!("return {code}"))
                .set_name("=eval")
                .into_function()
                .or_else(|_| lua.load(code.as_str()).set_name("=eval").into_function());
            Ok(match compiled {
                Ok(f) => {
                    pending.borrow_mut().insert(id, reply);
                    MultiValue::from_vec(vec![Value::Integer(id), Value::Function(f)])
                }
                Err(e) => {
                    let _ = reply.send(Err(e.to_string()));
                    MultiValue::new()
                }
            })
        })?;
        let reply = lua.create_function(move |_, args: MultiValue| {
            let mut args = args.into_iter();
            let id = match args.next() {
                Some(Value::Integer(id)) => id,
                _ => return Ok(()),
            };
            let ok = matches!(args.next(), Some(Value::Boolean(true)));
            let values: Vec<Value> = args.collect();
            if let Some(sender) = waiting.borrow_mut().remove(&id) {
                let _ = sender.send(if ok {
                    Ok(values.iter().map(describe).collect::<Vec<_>>().join(", "))
                } else {
                    Err(values.first().map_or_else(|| "error".into(), describe_plain))
                });
            }
            Ok(())
        })?;
        let main: Function = lua
            .load(
                "local take, reply = ...\n\
                 return function()\n\
                 while true do\n\
                 local id, f = take()\n\
                 if id then reply(id, pcall(f)) else wait() end\n\
                 end end",
            )
            .set_name("=repl")
            .call((take, reply))?;
        lua.sandbox(true)?;
        let main = lua.create_thread(main)?;
        Ok(Vm { lua, main, deadline, busy: Duration::ZERO })
    }

    /// Runs the mod until it waits, ends, fails or runs out of time.
    pub fn resume(&mut self) -> Step {
        if !self.main.is_resumable() {
            return Step::Finished;
        }
        let start = Instant::now();
        self.deadline.set(start + SLICE);
        let result = self.main.resume::<MultiValue>(());
        self.busy += start.elapsed();
        match result {
            Err(e) => Step::Failed(error_text(&e)),
            Ok(_) if !self.main.is_resumable() => Step::Finished,
            Ok(values) => match values.into_iter().next() {
                // `wait(ms)`.
                Some(v) => {
                    self.busy = Duration::ZERO;
                    let ms = match v {
                        Value::Integer(i) => i.clamp(0, i32::MAX.into()) as u32,
                        Value::Number(n) if n.is_finite() => n.clamp(0.0, i32::MAX as f64) as u32,
                        _ => 0,
                    };
                    Step::Wait(ms)
                }
                // The interrupt: out of time.
                None if self.busy >= RUNAWAY => Step::Failed(format!(
                    "ran for {} s without calling wait(): a loop needs wait() to let the game go on",
                    RUNAWAY.as_secs()
                )),
                None => Step::Preempted,
            },
        }
    }

    /// Memory the VM uses, in bytes.
    pub fn memory(&self) -> usize {
        self.lua.used_memory()
    }
}

/// A VM with the libraries, the natives and the mods' globals, and the
/// interrupt that keeps a slice short; not sandboxed yet.
fn base(name: &str, log: Rc<dyn Fn(&str)>) -> mlua::Result<(Lua, Rc<Cell<Instant>>)> {
    let lua = Lua::new_with(
        StdLib::COROUTINE
            | StdLib::TABLE
            | StdLib::STRING
            | StdLib::UTF8
            | StdLib::MATH
            | StdLib::BIT
            | StdLib::VECTOR
            | StdLib::OS,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY)?;
    let g = lua.globals();
    g.set(
        "print",
        lua.create_function(move |_, args: MultiValue| {
            log(&args.iter().map(describe_plain).collect::<Vec<_>>().join(" "));
            Ok(())
        })?,
    )?;
    // `wait(ms)`: back to the game until then; plain Lua, so it yields from
    // the mod's coroutine.
    let wait: Function = lua
        .load("local yield = coroutine.yield\nreturn function(ms) yield(ms or 0) end")
        .set_name("=wait")
        .eval()?;
    g.set("wait", wait)?;
    g.set("joaat", lua.create_function(|_, s: mlua::LuaString| Ok(natives::joaat(&s.to_str()?)))?)?;
    let m = lua.create_table()?;
    m.set("name", name)?;
    g.set("mod", m)?;

    // A table per namespace; a native's function is made the first time it
    // is looked up.
    for (ns, space) in natives::NAMESPACES.iter().enumerate() {
        let cache: RefCell<HashMap<String, Function>> = RefCell::default();
        let table = lua.create_table()?;
        let meta = lua.create_table()?;
        meta.set(
            "__index",
            lua.create_function(move |lua, (_, key): (Table, String)| {
                if let Some(f) = cache.borrow().get(&key) {
                    return Ok(Value::Function(f.clone()));
                }
                let Some(index) = natives::find(ns, &key) else { return Ok(Value::Nil) };
                let f = lua.create_function(move |lua, args: MultiValue| natives::call(lua, index, args))?;
                cache.borrow_mut().insert(key, f.clone());
                Ok(Value::Function(f))
            })?,
        )?;
        table.set_metatable(Some(meta))?;
        g.set(*space, table)?;
    }

    let deadline = Rc::new(Cell::new(Instant::now() + SLICE));
    let interrupt_deadline = deadline.clone();
    lua.set_interrupt(move |_| {
        Ok(if Instant::now() >= interrupt_deadline.get() { VmState::Yield } else { VmState::Continue })
    });
    Ok((lua, deadline))
}

/// `require("./menu")`: the mod's `menu.luau`, run once.
fn require(lua: &Lua, name: &str, dir: &Path) -> mlua::Result<Function> {
    let loaded: RefCell<HashMap<PathBuf, Value>> = RefCell::default();
    let (name, dir) = (name.to_string(), dir.to_path_buf());
    lua.create_function(move |lua, path: String| {
        let rel = path
            .strip_prefix("./")
            .filter(|r| !r.split('/').any(|p| p.is_empty() || p.starts_with('.')))
            .ok_or_else(|| mlua::Error::runtime(format!("require(\"{path}\"): a file of the mod, as \"./name\"")))?;
        let file = dir.join(format!("{rel}.luau"));
        if let Some(v) = loaded.borrow().get(&file) {
            return Ok(v.clone());
        }
        let source = std::fs::read_to_string(&file)
            .map_err(|e| mlua::Error::runtime(format!("require(\"{path}\"): {e}")))?;
        let value: Value = lua.load(source).set_name(format!("@{name}/{rel}.luau")).call(())?;
        let value = if value.is_nil() { Value::Boolean(true) } else { value };
        loaded.borrow_mut().insert(file, value.clone());
        Ok(value)
    })
}

/// An error as people read it: its message, and where in the mod's files it
/// happened when the message does not say (natives' errors).
pub fn error_text(e: &mlua::Error) -> String {
    let (message, traceback) = match e {
        mlua::Error::CallbackError { traceback, cause } => (error_text(cause), Some(traceback.as_str())),
        mlua::Error::RuntimeError(m) => (m.clone(), None),
        mlua::Error::SyntaxError { message, .. } => (message.clone(), None),
        mlua::Error::MemoryError(_) => ("out of memory (a mod may use 64 MB)".into(), None),
        other => (other.to_string(), None),
    };
    // `trainer/main.luau:12: in function ...`: the first frame in a mod's file.
    let at = traceback.and_then(|t| {
        t.lines().map(str::trim).find(|l| l.contains(".luau:")).and_then(|l| {
            let end = l.find(".luau:")? + 6;
            let digits = l[end..].chars().take_while(char::is_ascii_digit).count();
            Some(l[..end + digits].trim_start_matches('@').to_string())
        })
    });
    match at {
        Some(at) if !message.contains(".luau:") => format!("{at}: {message}"),
        _ => message,
    }
}

/// A value as the REPL shows it.
pub fn describe(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{:?}", s.to_string_lossy()),
        other => describe_plain(other),
    }
}

/// A value as `print` shows it: strings as they are.
fn describe_plain(v: &Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{n:.0}"),
        Value::Number(n) => format!("{n}"),
        Value::Vector(v) => format!("vector({}, {}, {})", short(v.x()), short(v.y()), short(v.z())),
        Value::String(s) => s.to_string_lossy(),
        Value::Table(t) => {
            let mut parts = Vec::new();
            for (i, pair) in t.pairs::<Value, Value>().enumerate() {
                if i == 16 {
                    parts.push("…".to_string());
                    break;
                }
                if let Ok((k, v)) = pair {
                    parts.push(match k {
                        Value::Integer(_) => describe(&v),
                        k => format!("{} = {}", describe_plain(&k), describe(&v)),
                    });
                }
            }
            format!("{{{}}}", parts.join(", "))
        }
        Value::Function(_) => "function".into(),
        Value::Error(e) => error_text(e),
        other => other.type_name().into(),
    }
}

fn short(x: f32) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
