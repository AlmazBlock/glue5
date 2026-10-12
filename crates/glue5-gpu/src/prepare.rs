//! The pipelines known before the game starts (the app's seed and this
//! device's recordings, see `pipeline.rs`), compiled on every core while the
//! start page shows how far it got, then handed to the renderer: the world
//! never shows with them missing.
//!
//! Compiling a pipeline the driver has not seen takes long on a phone
//! (~200 ms of CPU on an iPhone 13 Pro, 100 times a warm start's): the first
//! start waits minutes, later ones about a second. Metal keeps what it
//! compiled in its system cache, Vulkan in the pipeline cache saved in the
//! cache directory. Pipelines the game asks for that were never recorded
//! still compile in the background while playing; their draws are skipped
//! meanwhile.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use anyhow::Result;
use rustc_hash::FxHashMap;

use crate::pipeline::{self, Cell, Compiler, Recipe};
use crate::shader::{Library, Program, Stage};
use crate::{Device, Settings};

/// What the renderer takes over: the shaders, the compiler and the
/// pipelines compiled.
pub struct Prepared {
    pub library: Arc<Library>,
    pub compiler: Arc<Compiler>,
    pub pipes: FxHashMap<Arc<Recipe>, Arc<Cell>>,
}

/// Pipelines compiled so far, of how many.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// Opens the game's shaders and compiles the known pipelines, on all cores.
pub(crate) fn prepare(gpu: &Device, settings: &Settings, progress: &Progress) -> Result<Prepared> {
    let t0 = Instant::now();
    let library = Arc::new(Library::open(&settings.shader_dir)?);
    let compiler = Arc::new(crate::compiler(gpu, settings));
    let recipes = pipeline::known_recipes(settings.cache_dir.as_deref());
    progress.total.store(recipes.len(), Ordering::Relaxed);
    let cells: Vec<Arc<Cell>> = recipes.iter().map(|_| Arc::new(OnceLock::new())).collect();
    let next = AtomicUsize::new(0);
    let threads = glue5_runtime::cores();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(r) = recipes.get(i) else { break };
                    let pipe =
                        programs(&library, &gpu.device, r).and_then(|(vs, ps)| compiler.compile(&vs, ps.as_deref(), r));
                    let _ = cells[i].set(pipe);
                    progress.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    // The modules were needed to compile only; the game makes them again
    // for new pipelines (see `shader::Modules`).
    let mut modules = Vec::new();
    library.take_idle_modules(0, usize::MAX, &mut modules);
    drop(modules);
    let failed = cells.iter().filter(|c| matches!(c.get(), Some(None))).count();
    log::info!(
        "gpu: {} known pipelines prepared in {:.1} s ({failed} not usable, {threads} threads)",
        recipes.len(),
        t0.elapsed().as_secs_f64()
    );
    Ok(Prepared { library, compiler, pipes: recipes.into_iter().zip(cells).collect() })
}

/// The programs of a recipe, prepared as when the engine creates them and
/// made into the variants the renderer picks for what is bound (the same
/// objects: `Program::for_bindings` makes each once).
fn programs(library: &Library, device: &wgpu::Device, r: &Recipe) -> Option<(Arc<Program>, Option<Arc<Program>>)> {
    let program =
        |hash: u64, stage: Stage| library.slot(hash).get_or_init(|| library.prepare(device, hash, stage)).clone();
    let [vs_unfiltered, vs_depth, ps_unfiltered, ps_depth] = r.variant;
    let vs = program(r.vs, Stage::Vertex)?.for_bindings(device, vs_unfiltered & !vs_depth, vs_depth);
    let ps = match r.ps {
        Some(hash) => {
            let ps = program(hash, Stage::Pixel)?;
            let ps = if r.hud_mask { ps.for_hud_mask(device)? } else { ps };
            Some(ps.for_bindings(device, ps_unfiltered & !ps_depth, ps_depth))
        }
        None => None,
    };
    Some((vs, ps))
}
