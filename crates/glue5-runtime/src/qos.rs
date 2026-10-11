//! Thread priorities. On Apple platforms they are quality-of-service classes,
//! which also decide between performance and efficiency cores: the threads a
//! frame waits on are interactive, the engine's others just below.

#[derive(Clone, Copy, Debug)]
pub enum Qos {
    /// A frame waits on it (the engine's main thread, the renderer).
    Interactive,
    /// Work the game needs soon (engine workers, pipeline compiles).
    Initiated,
    /// Background work (logs).
    Utility,
}

/// Sets the calling thread's class.
pub fn set(qos: Qos) {
    #[cfg(target_vendor = "apple")]
    unsafe {
        use libc::qos_class_t::*;
        let class = match qos {
            Qos::Interactive => QOS_CLASS_USER_INTERACTIVE,
            Qos::Initiated => QOS_CLASS_USER_INITIATED,
            Qos::Utility => QOS_CLASS_UTILITY,
        };
        libc::pthread_set_qos_class_self_np(class, 0);
    }
    #[cfg(not(target_vendor = "apple"))]
    let _ = qos;
}
