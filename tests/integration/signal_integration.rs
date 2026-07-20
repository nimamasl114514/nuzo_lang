use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nuzo_compiler::{COMPILE_FINISHED_KEY, COMPILE_STARTED_KEY, compiler_bus};
use nuzo_signal::*;
use nuzo_vm::{GC_DID_COLLECT_KEY, GC_WILL_COLLECT_KEY, Gc};

#[test]
fn gc_signals_fire_in_order() {
    // 创建 GC 实例以获取其内部的 scoped SignalBus
    let gc = Gc::with_default_threshold();
    let bus = gc.bus().clone();

    let will_count = Arc::new(AtomicUsize::new(0));
    let did_count = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&will_count);
    let d = Arc::clone(&did_count);

    let sig_will =
        bus.get(&GC_WILL_COLLECT_KEY).expect("GC_WILL_COLLECT_KEY signal should be registered");
    let sig_did =
        bus.get(&GC_DID_COLLECT_KEY).expect("GC_DID_COLLECT_KEY signal should be registered");

    let conn1 = sig_will.connect(move |info: &GcWillCollectInfo| {
        assert_eq!(info.live_count, 42);
        assert_eq!(info.threshold, 100);
        w.fetch_add(1, Ordering::SeqCst);
    });
    let conn2 = sig_did.connect(move |info: &GcDidCollectInfo| {
        assert_eq!(info.freed_count, 10);
        assert_eq!(info.new_threshold, 120);
        d.fetch_add(1, Ordering::SeqCst);
    });
    sig_will.emit(&GcWillCollectInfo { live_count: 42, threshold: 100 });
    sig_did.emit(&GcDidCollectInfo {
        freed_count: 10,
        elapsed: std::time::Duration::from_millis(5),
        new_threshold: 120,
    });
    assert!(will_count.load(Ordering::SeqCst) >= 1);
    assert!(did_count.load(Ordering::SeqCst) >= 1);
    if let Ok(c) = conn1 {
        c.disconnect();
    }
    if let Ok(c) = conn2 {
        c.disconnect();
    }
}

#[test]
fn compiler_signals_accessible() {
    let bus = compiler_bus();
    let started = bus.get(&COMPILE_STARTED_KEY).unwrap();
    let finished = bus.get(&COMPILE_FINISHED_KEY).unwrap();
    assert_eq!(started.name(), "compile_started");
    assert_eq!(finished.name(), "compile_finished");
}

#[test]
fn compiler_signal_payload_validation() {
    let bus = compiler_bus();
    let started_info = Arc::new(std::sync::Mutex::new(None::<CompileStartedInfo>));
    let finished_info = Arc::new(std::sync::Mutex::new(None::<CompileFinishedInfo>));
    let si = Arc::clone(&started_info);
    let fi = Arc::clone(&finished_info);

    let conn1 = bus.get(&COMPILE_STARTED_KEY).unwrap().connect(move |info: &CompileStartedInfo| {
        *si.lock().unwrap() = Some(info.clone());
    });
    let conn2 =
        bus.get(&COMPILE_FINISHED_KEY).unwrap().connect(move |info: &CompileFinishedInfo| {
            *fi.lock().unwrap() = Some(info.clone());
        });

    bus.get(&COMPILE_STARTED_KEY).unwrap().emit(&CompileStartedInfo { source_len: 256 });
    bus.get(&COMPILE_FINISHED_KEY).unwrap().emit(&CompileFinishedInfo {
        success: true,
        chunk_size: Some(128),
        duration: std::time::Duration::from_millis(10),
        lex_duration: std::time::Duration::from_millis(2),
        parse_duration: std::time::Duration::from_millis(3),
        codegen_duration: std::time::Duration::from_millis(5),
    });

    let si = started_info.lock().unwrap().clone().unwrap();
    assert_eq!(si.source_len, 256);

    let fi = finished_info.lock().unwrap().clone().unwrap();
    assert!(fi.success);
    assert_eq!(fi.chunk_size, Some(128));

    if let Ok(c) = conn1 {
        c.disconnect();
    }
    if let Ok(c) = conn2 {
        c.disconnect();
    }
}

#[test]
fn compile_failure_signal_pairing() {
    let bus = compiler_bus();
    let started_called = Arc::new(AtomicUsize::new(0));
    let finished_success = Arc::new(AtomicUsize::new(0));
    let finished_failure = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&started_called);
    let fs = Arc::clone(&finished_success);
    let ff = Arc::clone(&finished_failure);

    let conn1 = bus.get(&COMPILE_STARTED_KEY).unwrap().connect(move |_| {
        sc.fetch_add(1, Ordering::SeqCst);
    });
    let conn2 =
        bus.get(&COMPILE_FINISHED_KEY).unwrap().connect(move |info: &CompileFinishedInfo| {
            if info.success {
                fs.fetch_add(1, Ordering::SeqCst);
            } else {
                ff.fetch_add(1, Ordering::SeqCst);
            }
        });

    bus.get(&COMPILE_STARTED_KEY).unwrap().emit(&CompileStartedInfo { source_len: 100 });
    bus.get(&COMPILE_FINISHED_KEY).unwrap().emit(&CompileFinishedInfo {
        success: false,
        chunk_size: None,
        duration: std::time::Duration::from_millis(2),
        lex_duration: std::time::Duration::ZERO,
        parse_duration: std::time::Duration::ZERO,
        codegen_duration: std::time::Duration::ZERO,
    });

    assert!(started_called.load(Ordering::SeqCst) >= 1);
    assert_eq!(finished_success.load(Ordering::SeqCst), 0);
    assert!(finished_failure.load(Ordering::SeqCst) >= 1);

    if let Ok(c) = conn1 {
        c.disconnect();
    }
    if let Ok(c) = conn2 {
        c.disconnect();
    }
}

#[test]
fn signal_bus_cross_module_registration() {
    let bus = SignalBus::scoped(BusScope::Gc);

    // 注册 GC 信号到 scoped bus
    let gc_signal = Signal::<GcWillCollectInfo>::named("gc_will_collect");
    let compiler_signal = Signal::<CompileStartedInfo>::named("compile_started");
    bus.register(&GC_WILL_COLLECT_KEY, &gc_signal).unwrap();
    bus.register(&COMPILE_STARTED_KEY, &compiler_signal).unwrap();
    let names = bus.list_signals();
    assert!(names.contains(&"GC_WILL_COLLECT_KEY"));
    assert!(names.contains(&"COMPILE_STARTED_KEY"));
    bus.clear();
}

#[test]
fn vm_observer_trait_available() {
    use nuzo_vm::{NoopVmObserver, VmObserver};
    let obs = NoopVmObserver;
    obs.on_will_execute(0, 0);
    use nuzo_signal::VmErrorInfo;
    obs.on_error(&VmErrorInfo { error_message: String::new(), opcode: None, ip: 0, call_depth: 0 });
}

#[test]
fn vm_observer_noop_default() {
    use nuzo_vm::{NoopVmObserver, VmObserver};
    // NoopVmObserver 的所有回调应该是空操作
    let obs = NoopVmObserver;
    obs.on_will_execute(42, 100);
}

#[test]
fn builtin_signal_accessible() {
    use nuzo_helpers::{BUILTIN_CALLED_KEY, BuiltinRegistry};
    let registry = BuiltinRegistry::new();
    let bus = registry.bus();
    let sig = bus.get(&BUILTIN_CALLED_KEY).unwrap();
    assert_eq!(sig.name(), "builtin_called");
}

#[test]
fn builtin_signal_payload_validation() {
    use nuzo_helpers::{BUILTIN_CALLED_KEY, BuiltinRegistry};

    let registry = BuiltinRegistry::new();
    let bus = registry.bus();
    let sig = bus.get(&BUILTIN_CALLED_KEY).unwrap();

    let captured = Arc::new(std::sync::Mutex::new(None::<BuiltinCallInfo>));
    let c = Arc::clone(&captured);

    let conn = sig.connect(move |info: &BuiltinCallInfo| {
        *c.lock().unwrap() = Some(BuiltinCallInfo { name: info.name, arg_count: info.arg_count });
    });

    sig.emit(&BuiltinCallInfo { name: "print", arg_count: 2 });

    let info = captured.lock().unwrap().clone().unwrap();
    assert_eq!(info.name, "print");
    assert_eq!(info.arg_count, 2);

    if let Ok(c) = conn {
        c.disconnect();
    }
}

/// 验证 ErrorCollector 作为 ErrorSink 的核心事件流，以及 VmObserver → ErrorSink 桥接模式。
///
/// 当 T3.1 (ErrorSinkObserver) 实现后，桥接部分可替换为 ErrorSinkObserver 的直连测试。
#[test]
fn error_collector_signal_driven_mode() {
    use nuzo_error::{ErrorCollector, ErrorEvent, ErrorSink};
    use nuzo_signal::VmErrorInfo;
    use nuzo_vm::VmObserver;

    // ── 1. ErrorCollector 作为 ErrorSink 的核心通道 ──
    let mut collector = ErrorCollector::new();
    collector.enable();

    collector.sink_error(
        ErrorEvent::new("signal-driven error".to_string())
            .with_opcode(7)
            .with_ip(50)
            .with_call_depth(1),
    );
    collector.sink_error(
        ErrorEvent::new("another error".to_string())
            .with_opcode(12)
            .with_ip(100)
            .with_call_depth(2),
    );

    assert_eq!(collector.sunk_pending(), 2, "sink 后应有 2 个待处理事件");

    let drained = collector.drain_sunk();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].message, "signal-driven error");
    assert_eq!(drained[0].opcode, Some(7));
    assert_eq!(drained[0].ip, 50);
    assert_eq!(drained[0].call_depth, 1);
    assert_eq!(drained[1].message, "another error");
    assert_eq!(drained[1].opcode, Some(12));
    assert_eq!(drained[1].ip, 100);
    assert_eq!(drained[1].call_depth, 2);
    assert_eq!(collector.sunk_pending(), 0, "drain 后队列应为空");

    // ── 2. VmObserver → ErrorSink 桥接模式 ──
    // T3.1 (ErrorSinkObserver) 将自动化此桥接，此处手动验证桥接语义正确。
    struct BridgeObserver {
        events: std::sync::Mutex<Vec<ErrorEvent>>,
    }
    impl ErrorSink for BridgeObserver {
        fn sink_error(&self, event: ErrorEvent) {
            self.events.lock().unwrap().push(event);
        }
    }
    impl VmObserver for BridgeObserver {
        fn on_error(&self, info: &VmErrorInfo) {
            self.sink_error(
                ErrorEvent::new(info.error_message.clone())
                    .with_opcode(info.opcode.unwrap_or(0))
                    .with_ip(info.ip)
                    .with_call_depth(info.call_depth),
            );
        }
    }

    let obs = BridgeObserver { events: std::sync::Mutex::new(Vec::new()) };
    obs.on_error(&VmErrorInfo {
        error_message: "observer error".to_string(),
        opcode: Some(42),
        ip: 200,
        call_depth: 3,
    });

    let captured = obs.events.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].message, "observer error");
    assert_eq!(captured[0].opcode, Some(42));
    assert_eq!(captured[0].ip, 200);
    assert_eq!(captured[0].call_depth, 3);
}
