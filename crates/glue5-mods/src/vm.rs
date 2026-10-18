//! A mod's Luau VM: sandboxed, with the natives as namespace tables and
//! `wait`, its main chunk running as a coroutine the engine resumes once
//! per wait.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaOptions, MultiValue, StdLib, Table, Thread, Value, VmState};

use crate::natives;

/// How long a mod may run before it yields to the game for this frame.
const SLICE: Duration = Duration::from_millis(4);
/// How long a mod may run without calling `wait` before it is stopped.
const RUNAWAY: Duration = Duration::from_secs(2);
/// Memory a mod's VM may use.
const MEMORY: usize = 64 << 20;

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
        let lua = Lua::new_with(
            StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH | StdLib::BIT | StdLib::VECTOR,
            LuaOptions::default(),
        )?;
        lua.set_memory_limit(MEMORY)?;
        let g = lua.globals();

        let print_log = log.clone();
        g.set(
            "print",
            lua.create_function(move |_, args: MultiValue| {
                let text = args.iter().map(|v| v.to_string().unwrap_or_else(|_| "?".into())).collect::<Vec<_>>();
                print_log(&text.join(" "));
                Ok(())
            })?,
        )?;
        // `wait(ms)`: back to the game until then; plain Lua, so it yields
        // from the mod's coroutine.
        let wait: Function = lua
            .load("local yield = coroutine.yield\nreturn function(ms) yield(ms or 0) end")
            .set_name("=wait")
            .eval()?;
        g.set("wait", wait)?;
        g.set("joaat", lua.create_function(|_, s: mlua::LuaString| Ok(natives::joaat(&s.to_str()?)))?)?;
        let m = lua.create_table()?;
        m.set("name", name)?;
        g.set("mod", m)?;

        // A table per namespace; a native's function is made the first time
        // it is looked up.
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

        // The mod's code starts once the player is in the game (not on
        // the loading screens, where the player's ped is a stand-in).
        let ready: Function = lua
            .load(
                "local yield = coroutine.yield\n\
                 return function(main) return function(...)\n\
                 while dlc.getIsLoadingScreenActive() or not camera.isScreenFadedIn() or not player.isPlayerPlaying(player.playerId()) do yield(0) end\n\
                 return main(...)\n\
                 end end",
            )
            .set_name("=ready")
            .eval()?;

        let deadline = Rc::new(Cell::new(Instant::now() + SLICE));
        let interrupt_deadline = deadline.clone();
        lua.set_interrupt(move |_| {
            Ok(if Instant::now() >= interrupt_deadline.get() { VmState::Yield } else { VmState::Continue })
        });
        lua.sandbox(true)?;

        let path = dir.join("main.luau");
        let source = std::fs::read_to_string(&path)
            .map_err(|e| mlua::Error::runtime(format!("reading {}: {e}", path.display())))?;
        let chunk = lua.load(source).set_name(format!("@{name}/main.luau")).into_function()?;
        let main = lua.create_thread(ready.call::<Function>(chunk)?)?;
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
            Err(e) => Step::Failed(e.to_string()),
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
