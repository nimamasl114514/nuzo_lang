//! Plugin trait for extending Nuzo.

use nuzo_helpers::builtins::BuiltinRegistry;
use nuzo_signal::SignalBus;

pub trait NuzoPlugin {
    fn name(&self) -> &str;
    fn register_signals(&self, _bus: &SignalBus) {}
    fn register_builtins(&self, _registry: &mut BuiltinRegistry) {}
    fn on_start(&mut self) {}
    fn on_stop(&mut self) {}
}
