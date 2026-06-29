//! Output routing: fan processed audio out to one or more sinks.

use crate::types::Sample;

/// A destination for processed audio — a monitor device, the VB-Cable input,
/// and (later) a native virtual driver. Kept behind a trait so the set of
/// outputs is swappable without touching the engine, which is what lets us
/// move off VB-Cable to a bundled driver down the road.
pub trait Sink: Send {
    fn write(&mut self, buffer: &[Sample]);
}

/// Fans a single processed buffer out to every registered sink. Phase 1 wires
/// up two: the low-latency monitor and the VB-Cable output.
#[derive(Default)]
pub struct Router {
    sinks: Vec<Box<dyn Sink>>,
}

impl Router {
    pub fn new() -> Self {
        Self { sinks: Vec::new() }
    }

    pub fn add(&mut self, sink: Box<dyn Sink>) {
        self.sinks.push(sink);
    }

    pub fn broadcast(&mut self, buffer: &[Sample]) {
        for sink in &mut self.sinks {
            sink.write(buffer);
        }
    }

    pub fn len(&self) -> usize {
        self.sinks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct CollectSink(Arc<Mutex<Vec<Sample>>>);

    impl Sink for CollectSink {
        fn write(&mut self, buffer: &[Sample]) {
            self.0.lock().unwrap().extend_from_slice(buffer);
        }
    }

    #[test]
    fn broadcast_reaches_every_sink() {
        let monitor = CollectSink::default();
        let cable = CollectSink::default();

        let mut router = Router::new();
        router.add(Box::new(monitor.clone()));
        router.add(Box::new(cable.clone()));

        router.broadcast(&[0.1, 0.2, 0.3]);

        assert_eq!(*monitor.0.lock().unwrap(), vec![0.1, 0.2, 0.3]);
        assert_eq!(*cable.0.lock().unwrap(), vec![0.1, 0.2, 0.3]);
    }
}
