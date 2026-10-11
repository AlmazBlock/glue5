//! Command buffers to the GPU, off the recording thread. wgpu encodes a pass
//! for the GPU when its command encoder is finished, which costs more than
//! recording it: each batch (what the recording thread hands over at a
//! flush) is finished by one of a few encoding threads while recording goes
//! on, and submitted in order by the submit thread, which then does what
//! must follow a submit: staging memory back to the pool, readbacks mapped,
//! destroyed objects released, the overlay drawn and the frame presented.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::done::{Done, Staging, Work};
use crate::present::Output;
use crate::uploads::FreeChunks;

/// Frames submitted but not finished by the GPU, at most.
const FRAMES_IN_FLIGHT: usize = 2;
/// Batches handed over but not submitted, at most: the recording thread
/// waits beyond (about a frame's worth).
const PENDING: usize = 8;
const ENCODE_THREADS: usize = 2;

/// What a flush hands over.
pub struct Batch {
    /// Uploads first, then the commands.
    pub encoders: Vec<wgpu::CommandEncoder>,
    /// Staging chunks the uploads copy from.
    pub staging: Vec<wgpu::Buffer>,
    /// Buffers to map once submitted, and the work their contents feed.
    pub maps: Vec<(wgpu::Buffer, Box<dyn FnOnce(Staging) -> Work + Send>)>,
    /// Work for once the GPU is done with the batch.
    pub works: Vec<Work>,
    /// Objects the batch uses that the engine has destroyed.
    pub graves: Vec<Grave>,
    pub present: Option<Present>,
    /// The batch ends a frame (paces frames in flight, window or not).
    pub frame: bool,
}

/// A back buffer to show after its batch.
pub struct Present {
    /// Its colour as stored (sRGB formats read raw).
    pub view: wgpu::TextureView,
    pub texture: wgpu::Texture,
    pub width: u32,
    pub height: u32,
}

pub enum Grave {
    Texture(wgpu::Texture),
    Buffer(wgpu::Buffer),
}

/// Times, in nanoseconds, since the statistics last took them.
#[derive(Default)]
pub struct SubmitStats {
    pub encode_ns: AtomicU64,
    pub submit_ns: AtomicU64,
    /// Waiting for the GPU to finish older frames.
    pub gpu_wait_ns: AtomicU64,
    /// Waiting for the window's next drawable.
    pub drawable_ns: AtomicU64,
    /// Presenting it.
    pub present_ns: AtomicU64,
    /// Releasing destroyed objects.
    pub release_ns: AtomicU64,
}

impl SubmitStats {
    pub fn take(&self) -> [Duration; 6] {
        let t = |a: &AtomicU64| Duration::from_nanos(a.swap(0, Ordering::Relaxed));
        [
            t(&self.encode_ns),
            t(&self.submit_ns),
            t(&self.gpu_wait_ns),
            t(&self.drawable_ns),
            t(&self.present_ns),
            t(&self.release_ns),
        ]
    }
}

type Encoded = Receiver<Vec<wgpu::CommandBuffer>>;
type Job = (Vec<wgpu::CommandEncoder>, Sender<Vec<wgpu::CommandBuffer>>);

pub struct Submitter {
    jobs: Sender<Job>,
    batches: SyncSender<(Batch, Encoded)>,
    pub stats: Arc<SubmitStats>,
}

impl Submitter {
    pub fn start(
        device: wgpu::Device,
        queue: wgpu::Queue,
        done: Done,
        free: FreeChunks,
        output: Option<Output>,
    ) -> Submitter {
        let stats = Arc::new(SubmitStats::default());
        let (jobs, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..ENCODE_THREADS {
            let (rx, stats) = (rx.clone(), stats.clone());
            std::thread::Builder::new()
                .name(format!("gpu encode {i}"))
                .spawn(move || {
                    glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
                    loop {
                        let job = rx.lock().unwrap().recv();
                        let Ok((encoders, back)) = job else { return };
                        let t = Instant::now();
                        let buffers: Vec<_> = encoders.into_iter().map(|e| e.finish()).collect();
                        stats.encode_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
                        let _ = back.send(buffers);
                    }
                })
                .expect("spawning a GPU encoding thread");
        }
        let (batches, rx) = sync_channel(PENDING);
        let thread_stats = stats.clone();
        std::thread::Builder::new()
            .name("gpu submit".into())
            .spawn(move || {
                glue5_runtime::qos::set(glue5_runtime::qos::Qos::Interactive);
                run(device, queue, done, free, output, rx, thread_stats)
            })
            .expect("spawning the GPU submit thread");
        Submitter { jobs, batches, stats }
    }

    /// Hands a batch over: encoded now on another thread, submitted after
    /// the batches before it.
    pub fn send(&self, mut batch: Batch) {
        let (back, encoded) = channel();
        let encoders = std::mem::take(&mut batch.encoders);
        if encoders.is_empty() {
            let _ = back.send(Vec::new());
        } else {
            let _ = self.jobs.send((encoders, back));
        }
        let _ = self.batches.send((batch, encoded));
    }
}

fn run(
    device: wgpu::Device,
    queue: wgpu::Queue,
    done: Done,
    free: FreeChunks,
    mut output: Option<Output>,
    rx: Receiver<(Batch, Encoded)>,
    stats: Arc<SubmitStats>,
) {
    let mut in_flight: VecDeque<wgpu::SubmissionIndex> = VecDeque::new();
    while let Ok((batch, encoded)) = rx.recv() {
        let Ok(mut buffers) = encoded.recv() else { return };
        let mut frame = None;
        if let (Some(p), Some(out)) = (&batch.present, output.as_mut()) {
            let t = Instant::now();
            let acquired = out.acquire(&device);
            stats.drawable_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            if let Some((surface, format)) = acquired {
                buffers.extend(out.compose(&device, &queue, &surface, format, Some(p)));
                frame = Some(surface);
            }
        }
        let t = Instant::now();
        let submission = queue.submit(buffers);
        crate::uploads::recycle(batch.staging, &free);
        let mut works = batch.works;
        for (buffer, make) in batch.maps {
            works.push(make(Staging::map(buffer)));
        }
        done.after(submission.clone(), works);
        stats.submit_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if let Some(surface) = frame {
            let t = Instant::now();
            queue.present(surface);
            stats.present_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
        let t = Instant::now();
        for grave in batch.graves {
            match grave {
                Grave::Texture(t) => t.destroy(),
                Grave::Buffer(b) => b.destroy(),
            }
        }
        stats.release_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        if batch.frame {
            in_flight.push_back(submission);
            if in_flight.len() > FRAMES_IN_FLIGHT {
                let oldest = in_flight.pop_front().unwrap();
                let t = Instant::now();
                let _ = device.poll(wgpu::PollType::Wait {
                    submission_index: Some(oldest),
                    timeout: Some(Duration::from_secs(5)),
                });
                stats.gpu_wait_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
            }
        }
    }
}
