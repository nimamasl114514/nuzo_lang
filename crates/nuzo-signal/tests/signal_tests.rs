use nuzo_signal::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// 测试专用 SignalKey 常量（与 BusScope::Custom("test") 配对）
const BUS_TEST_KEY: SignalKey<i32> = SignalKey::new("bus_test_signal", BusScope::Custom("test"));
const DUP_KEY: SignalKey<()> = SignalKey::new("dup_signal", BusScope::Custom("test"));
const TYPE_TEST_KEY_I32: SignalKey<i32> = SignalKey::new("type_test", BusScope::Custom("test"));
const TYPE_TEST_KEY_STRING: SignalKey<String> =
    SignalKey::new("type_test", BusScope::Custom("test"));
const NONEXISTENT_KEY: SignalKey<()> = SignalKey::new("nonexistent", BusScope::Custom("test"));
const SIG_A_KEY: SignalKey<()> = SignalKey::new("sig_a", BusScope::Custom("test"));
const SIG_B_KEY: SignalKey<i32> = SignalKey::new("sig_b", BusScope::Custom("test"));
const CLEAR_KEY: SignalKey<()> = SignalKey::new("clear_test", BusScope::Custom("test"));

#[test]
fn signal_named_creates_signal_with_name() {
    let signal: Signal<i32> = Signal::named("test_signal");
    assert_eq!(signal.name(), "test_signal");
    assert_eq!(signal.slot_count(), 0);
    assert!(signal.is_empty());
}

#[test]
fn connect_increments_slot_count() {
    let signal: Signal<()> = Signal::named("count_test");
    assert_eq!(signal.slot_count(), 0);
    let conn = signal.connect(|_| {}).unwrap();
    assert_eq!(signal.slot_count(), 1);
    assert!(!signal.is_empty());
    assert!(conn.is_connected());
}

#[test]
fn emit_calls_connected_slots() {
    let signal: Signal<i32> = Signal::named("emit_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let c1 = Arc::clone(&counter);
    let _conn = signal
        .connect(move |v| {
            c1.fetch_add(*v as usize, Ordering::SeqCst);
        })
        .unwrap();
    let result = signal.emit(&42);
    assert_eq!(result.invoked_count, 1);
    assert!(result.is_ok());
    assert_eq!(counter.load(Ordering::SeqCst), 42);
}

#[test]
fn emit_empty_signal_returns_zero_invoked() {
    let signal: Signal<()> = Signal::named("empty_emit");
    let result = signal.emit(&());
    assert_eq!(result.invoked_count, 0);
    assert!(result.is_ok());
}

#[test]
fn disconnect_removes_slot() {
    let signal: Signal<()> = Signal::named("disconnect_test");
    let conn = signal.connect(|_| {}).unwrap();
    assert_eq!(signal.slot_count(), 1);
    assert!(conn.is_connected());
    conn.disconnect();
    assert_eq!(signal.slot_count(), 0);
}

#[test]
fn disconnect_idempotent() {
    let signal: Signal<()> = Signal::named("idempotent_test");
    let conn1 = signal.connect(|_| {}).unwrap();
    let conn2 = signal.connect(|_| {}).unwrap();
    assert_eq!(signal.slot_count(), 2);
    conn1.disconnect();
    assert_eq!(signal.slot_count(), 1);
    conn2.disconnect();
    assert_eq!(signal.slot_count(), 0);
}

#[test]
fn disconnect_all_removes_all_slots() {
    let signal: Signal<()> = Signal::named("disconnect_all_test");
    let _c1 = signal.connect(|_| {}).unwrap();
    let _c2 = signal.connect(|_| {}).unwrap();
    let _c3 = signal.connect(|_| {}).unwrap();
    assert_eq!(signal.slot_count(), 3);
    signal.disconnect_all();
    assert_eq!(signal.slot_count(), 0);
    assert!(signal.is_empty());
}

#[test]
fn disconnect_by_group_removes_matching_slots() {
    let signal: Signal<()> = Signal::named("group_test");
    let _c1 = signal.connect_with_group(|_| {}, "debug").unwrap();
    let _c2 = signal.connect(|_| {}).unwrap();
    let _c3 = signal.connect_with_group(|_| {}, "debug").unwrap();
    let _c4 = signal.connect_with_group(|_| {}, "profiling").unwrap();
    assert_eq!(signal.slot_count(), 4);
    signal.disconnect_by_group("debug");
    assert_eq!(signal.slot_count(), 2);
}

#[test]
fn priority_ordering() {
    let signal: Signal<()> = Signal::named("priority_test");
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let o1 = Arc::clone(&order);
    let _c1 = signal
        .connect_with_priority(
            move |_| {
                o1.lock().unwrap().push("low");
            },
            Priority::Low(0),
        )
        .unwrap();
    let o2 = Arc::clone(&order);
    let _c2 = signal
        .connect_with_priority(
            move |_| {
                o2.lock().unwrap().push("normal");
            },
            Priority::Normal,
        )
        .unwrap();
    let o3 = Arc::clone(&order);
    let _c3 = signal
        .connect_with_priority(
            move |_| {
                o3.lock().unwrap().push("high");
            },
            Priority::High(0),
        )
        .unwrap();
    signal.emit(&());
    assert_eq!(*order.lock().unwrap(), vec!["high", "normal", "low"]);
}

#[test]
fn emit_isolates_panic() {
    let signal: Signal<()> = Signal::named("panic_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let _c1 = signal
        .connect(|_| {
            panic!("slot panic!");
        })
        .unwrap();
    let c = Arc::clone(&counter);
    let _c2 = signal
        .connect(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let result = signal.emit(&());
    assert_eq!(result.invoked_count, 1);
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].message.contains("slot panic!"));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn emit_stop_on_error() {
    let signal: Signal<()> = Signal::named("stop_on_error_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let _c1 = signal
        .connect(|_| {
            panic!("first panic");
        })
        .unwrap();
    let c = Arc::clone(&counter);
    let _c2 = signal
        .connect(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    let result = signal.emit_with_options(&(), EmitOptions { on_error: ErrorPolicy::Stop });
    assert_eq!(result.invoked_count, 0);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 0);
}

#[test]
fn connection_id_returns_strong_type() {
    let signal: Signal<()> = Signal::named("conn_id_test");
    let conn = signal.connect(|_| {}).unwrap();
    let id = conn.id();
    assert_eq!(id.as_u64(), 1);
}

#[test]
fn connection_is_connected_reflects_state() {
    let signal: Signal<()> = Signal::named("conn_state_test");
    let conn = signal.connect(|_| {}).unwrap();
    assert!(conn.is_connected());
    conn.disconnect();
}

#[test]
fn signal_bus_register_and_find() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let signal: Signal<i32> = Signal::named("bus_test_signal");
    bus.register(&BUS_TEST_KEY, &signal).unwrap();
    let found = bus.get(&BUS_TEST_KEY).unwrap();
    assert_eq!(found.name(), "bus_test_signal");
}

#[test]
fn signal_bus_duplicate_registration_fails() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let signal: Signal<()> = Signal::named("dup_signal");
    bus.register(&DUP_KEY, &signal).unwrap();
    let result = bus.register(&DUP_KEY, &signal);
    assert!(matches!(result, Err(SignalError::AlreadyRegistered { .. })));
}

#[test]
fn signal_bus_type_mismatch() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let signal: Signal<i32> = Signal::named("type_test");
    bus.register(&TYPE_TEST_KEY_I32, &signal).unwrap();
    let result = bus.get(&TYPE_TEST_KEY_STRING);
    assert!(matches!(result, Err(SignalError::TypeMismatch { .. })));
}

#[test]
fn signal_bus_not_found() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let result = bus.get(&NONEXISTENT_KEY);
    assert!(matches!(result, Err(SignalError::NotFound { .. })));
}

#[test]
fn signal_bus_list_signals() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let s1: Signal<()> = Signal::named("sig_a");
    let s2: Signal<i32> = Signal::named("sig_b");
    bus.register(&SIG_A_KEY, &s1).unwrap();
    bus.register(&SIG_B_KEY, &s2).unwrap();
    let names = bus.list_signals();
    assert!(names.contains(&"sig_a"));
    assert!(names.contains(&"sig_b"));
}

#[test]
fn signal_bus_clear() {
    let bus = SignalBus::scoped(BusScope::Custom("test"));
    let signal: Signal<()> = Signal::named("clear_test");
    bus.register(&CLEAR_KEY, &signal).unwrap();
    bus.clear();
    assert!(bus.list_signals().is_empty());
}

#[test]
fn signal_log_enable_disable() {
    let log = SignalLog::global();
    log.disable();
    assert!(!log.is_enabled());
    log.enable();
    assert!(log.is_enabled());
    log.disable();
}

#[test]
fn concurrent_connect_emit_disconnect() {
    use std::thread;
    let signal: Signal<i32> = Signal::named("concurrent_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];
    for _ in 0..4 {
        let sig = signal.clone_handle();
        let c = Arc::clone(&counter);
        handles.push(thread::spawn(move || {
            let conn = sig
                .connect(move |v| {
                    c.fetch_add(*v as usize, Ordering::SeqCst);
                })
                .unwrap();
            sig.emit(&1);
            conn.disconnect();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(signal.slot_count(), 0);
}

#[test]
fn connect_once_auto_disconnects_after_first_emit() {
    let signal: Signal<i32> = Signal::named("once_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    let conn = signal
        .connect_once(move |v| {
            c.fetch_add(*v as usize, Ordering::SeqCst);
        })
        .unwrap();

    assert!(conn.is_connected());
    assert_eq!(signal.slot_count(), 1);

    // 首次 emit：槽位应被调用
    let result = signal.emit(&10);
    assert_eq!(result.invoked_count, 1);
    assert_eq!(counter.load(Ordering::SeqCst), 10);

    // once 槽位应已被自动移除
    assert_eq!(signal.slot_count(), 0);
    assert!(!conn.is_connected());

    // 二次 emit：槽位不应被调用
    let result = signal.emit(&20);
    assert_eq!(result.invoked_count, 0);
    assert_eq!(counter.load(Ordering::SeqCst), 10); // 计数器不变
}

#[test]
fn connect_once_panic_does_not_auto_remove() {
    let signal: Signal<()> = Signal::named("once_panic_test");
    let counter = Arc::new(AtomicUsize::new(0));
    let _c1 = signal
        .connect_once(|_| {
            panic!("once panic!");
        })
        .unwrap();
    let c = Arc::clone(&counter);
    let _c2 = signal
        .connect(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

    let result = signal.emit(&());
    assert_eq!(result.invoked_count, 1); // 只有第二个槽位成功
    assert_eq!(result.errors.len(), 1);
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // once 槽位因 panic 未被移除，仍在列表中
    assert_eq!(signal.slot_count(), 2);
}

#[test]
fn recursive_emit_returns_warning() {
    let signal: Signal<()> = Signal::named("recursive_test");
    let counter = Arc::new(AtomicUsize::new(0));

    // 槽位中再次 emit 同一信号（递归）
    let sig_clone = signal.clone_handle();
    let c = Arc::clone(&counter);
    let _conn = signal
        .connect(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            // 递归 emit 应被保护，返回带警告的 EmitResult
            let recursive_result = sig_clone.emit(&());
            assert!(!recursive_result.is_ok());
            assert!(recursive_result.errors[0].message.contains("recursive emit"));
        })
        .unwrap();

    let result = signal.emit(&());
    assert_eq!(result.invoked_count, 1);
    assert!(result.is_ok());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[test]
fn emit_result_total_count_matches_snapshot() {
    let signal: Signal<()> = Signal::named("total_count_test");
    let _c1 = signal.connect(|_| {}).unwrap();
    let _c2 = signal.connect(|_| {}).unwrap();
    let _c3 = signal.connect(|_| {}).unwrap();

    let result = signal.emit(&());
    assert_eq!(result.total_count, 3);
    assert_eq!(result.invoked_count, 3);
}

#[test]
fn disconnect_all_updates_connected_flags() {
    let signal: Signal<()> = Signal::named("disconnect_all_flags_test");
    let conn1 = signal.connect(|_| {}).unwrap();
    let conn2 = signal.connect(|_| {}).unwrap();

    assert!(conn1.is_connected());
    assert!(conn2.is_connected());

    signal.disconnect_all();

    assert!(!conn1.is_connected());
    assert!(!conn2.is_connected());
}

#[test]
fn disconnect_by_group_updates_connected_flags() {
    let signal: Signal<()> = Signal::named("group_flags_test");
    let conn1 = signal.connect_with_group(|_| {}, "debug").unwrap();
    let conn2 = signal.connect(|_| {}).unwrap();
    let conn3 = signal.connect_with_group(|_| {}, "debug").unwrap();

    assert!(conn1.is_connected());
    assert!(conn2.is_connected());
    assert!(conn3.is_connected());

    signal.disconnect_by_group("debug");

    assert!(!conn1.is_connected());
    assert!(conn2.is_connected()); // 不在 debug 组，应保持连接
    assert!(!conn3.is_connected());
}

// =========================================================================
// BusScope 边界测试
// =========================================================================

#[cfg(test)]
mod scope_tests {
    use nuzo_signal::*;

    /// 验证不同作用域的总线实例互相隔离：
    /// - GC 总线只能查找注册在 GC 总线上的信号
    /// - Compiler 总线只能查找注册在 Compiler 总线上的信号
    /// - 跨 bus 实例查找应返回 NotFound
    #[test]
    fn test_scope_gc_vs_compiler_isolation() {
        let gc_bus = SignalBus::scoped(BusScope::Gc);
        let compiler_bus = SignalBus::scoped(BusScope::Compiler);

        declare_signal!(SCOPE_GC_KEY, i32, BusScope::Gc);
        declare_signal!(SCOPE_COMPILER_KEY, String, BusScope::Compiler);

        let gc_sig = Signal::<i32>::named("SCOPE_GC_KEY");
        gc_bus.register(&SCOPE_GC_KEY, &gc_sig).unwrap();

        let compiler_sig = Signal::<String>::named("SCOPE_COMPILER_KEY");
        compiler_bus.register(&SCOPE_COMPILER_KEY, &compiler_sig).unwrap();

        // 各自总线能查到自己的信号
        assert!(gc_bus.get(&SCOPE_GC_KEY).is_ok());
        assert!(compiler_bus.get(&SCOPE_COMPILER_KEY).is_ok());

        // 跨总线查找应返回 NotFound（不同实例之间没有共享注册表）
        assert!(gc_bus.get(&SCOPE_COMPILER_KEY).is_err());
        assert!(compiler_bus.get(&SCOPE_GC_KEY).is_err());
    }

    /// 验证 Custom 作用域的正确构造和 scope() 返回值
    #[test]
    fn test_scope_custom_identity() {
        let bus = SignalBus::scoped(BusScope::Custom("my_plugin"));
        assert_eq!(bus.scope(), BusScope::Custom("my_plugin"));
    }

    /// 验证同一总线上重复注册相同 key 返回 AlreadyRegistered
    #[test]
    fn test_scope_duplicate_register() {
        let bus = SignalBus::scoped(BusScope::Gc);
        declare_signal!(SCOPE_DUP_KEY, i32, BusScope::Gc);
        let sig1 = Signal::<i32>::named("SCOPE_DUP_KEY");
        bus.register(&SCOPE_DUP_KEY, &sig1).unwrap();

        // 再次注册相同 key 应返回 AlreadyRegistered
        let result = bus.register(&SCOPE_DUP_KEY, &sig1);
        assert!(matches!(result, Err(SignalError::AlreadyRegistered { .. })));
    }

    /// 验证两个同名但不同 Custom scope 的总线实例完全独立
    #[test]
    fn test_scope_custom_isolation() {
        let bus_a = SignalBus::scoped(BusScope::Custom("a"));
        let bus_b = SignalBus::scoped(BusScope::Custom("b"));

        declare_signal!(SCOPE_ISOLATION_A_KEY, i32, BusScope::Custom("a"));
        let sig = Signal::<i32>::named("SCOPE_ISOLATION_A_KEY");
        bus_a.register(&SCOPE_ISOLATION_A_KEY, &sig).unwrap();

        assert!(bus_a.get(&SCOPE_ISOLATION_A_KEY).is_ok());
        // bus_b 没有注册此信号，查找应失败
        assert!(bus_b.get(&SCOPE_ISOLATION_A_KEY).is_err());
    }

    /// 验证 BusScope 的 Display 实现输出正确
    #[test]
    fn test_scope_display() {
        assert_eq!(format!("{}", BusScope::Gc), "gc");
        assert_eq!(format!("{}", BusScope::Compiler), "compiler");
        assert_eq!(format!("{}", BusScope::Builtin), "builtin");
        assert_eq!(format!("{}", BusScope::Custom("plugin:x")), "custom:plugin:x");
    }

    /// 验证 BusScope 的 PartialEq/Eq 语义正确
    #[test]
    fn test_scope_equality() {
        assert_eq!(BusScope::Gc, BusScope::Gc);
        assert_ne!(BusScope::Gc, BusScope::Compiler);
        assert_eq!(BusScope::Custom("a"), BusScope::Custom("a"));
        assert_ne!(BusScope::Custom("a"), BusScope::Custom("b"));
    }
}

// =========================================================================
// SignalKey 边界测试
// =========================================================================

#[cfg(test)]
mod signal_key_tests {
    use nuzo_signal::*;

    /// 验证 SignalKey 可在 const 上下文中构造
    #[test]
    fn test_key_const_constructible() {
        const MY_KEY: SignalKey<i32> = SignalKey::new("my_key", BusScope::Gc);
        assert_eq!(MY_KEY.name(), "my_key");
        assert_eq!(MY_KEY.scope(), BusScope::Gc);
    }

    /// 验证 SignalKey 的相等性语义：
    /// - 同名同 scope 相等
    /// - 不同名不等
    /// - 不同 scope 不等
    #[test]
    fn test_key_equality() {
        let key1 = SignalKey::<i32>::new("test", BusScope::Gc);
        let key2 = SignalKey::<i32>::new("test", BusScope::Gc);
        let key3 = SignalKey::<i32>::new("other", BusScope::Gc);
        let key4 = SignalKey::<i32>::new("test", BusScope::Compiler);

        assert_eq!(key1, key2); // 同名同 scope
        assert_ne!(key1, key3); // 不同名
        assert_ne!(key1, key4); // 不同 scope
    }

    /// 验证 declare_signal! 宏正确展开 name 和 scope
    #[test]
    fn test_macro_expansion() {
        declare_signal!(KEY_MACRO_TEST, String, BusScope::Builtin);
        assert_eq!(KEY_MACRO_TEST.name(), "KEY_MACRO_TEST");
        assert_eq!(KEY_MACRO_TEST.scope(), BusScope::Builtin);
    }

    /// 验证 SignalKey 的 Copy 语义：复制后两个变量相等且独立
    #[test]
    fn test_key_copy_semantics() {
        let key1 = SignalKey::<i32>::new("copy_test", BusScope::Gc);
        let key2 = key1; // Copy trait
        assert_eq!(key1, key2);
        // 两个变量仍然相等（Copy 是位拷贝）
        assert_eq!(key1.name(), "copy_test");
        assert_eq!(key2.name(), "copy_test");
    }

    /// 验证 SignalKey 的 Hash 一致性：
    /// 相同的 key 两次 hash 应产生相同结果
    #[test]
    fn test_key_hash_consistency() {
        use std::collections::HashSet;
        let key1 = SignalKey::<i32>::new("hash_test", BusScope::Gc);
        let key2 = SignalKey::<i32>::new("hash_test", BusScope::Gc);
        let key3 = SignalKey::<i32>::new("hash_test", BusScope::Compiler);

        let mut set = HashSet::new();
        set.insert(key1);
        assert!(set.contains(&key2)); // 同名同 scope 应匹配
        assert!(!set.contains(&key3)); // 不同 scope 不匹配
    }

    /// 验证不同泛型参数的 SignalKey 可以共存（TypeId 隔离）
    #[test]
    fn test_key_different_types_coexist() {
        let key_i32 = SignalKey::<i32>::new("shared_name", BusScope::Gc);
        let key_string = SignalKey::<String>::new("shared_name", BusScope::Gc);

        // 同名同 scope 但不同泛型参数，它们是不同的类型
        assert_eq!(key_i32.name(), key_string.name());
        assert_eq!(key_i32.scope(), key_string.scope());
        // 注意：由于泛型参数不同，无法直接比较 SignalKey<i32> == SignalKey<String>
        // 但它们可以注册到同一个 bus 中（因为 TypeId 不同）
        let bus = SignalBus::scoped(BusScope::Gc);
        let sig_i32 = Signal::<i32>::named("shared_name");
        let sig_string = Signal::<String>::named("shared_name");
        bus.register(&key_i32, &sig_i32).unwrap();
        bus.register(&key_string, &sig_string).unwrap(); // 同名不同类型，允许注册
    }
}

// =========================================================================
// ASB (Adaptive Slot Batching) 测试
// =========================================================================

#[cfg(test)]
mod asb_tests {
    use nuzo_signal::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 验证 SlotStats 公共 API 可正常使用
    #[test]
    fn test_slot_stats_public_api() {
        let mut s = SlotStats::new();
        assert_eq!(s.tier, SlotTier::Cold);
        assert_eq!(s.call_count, 0);

        s.record_call();
        assert_eq!(s.call_count, 1);
        assert!(s.decayed_score > 0.0);

        s.decay();
        // 衰减后分数应降低
    }

    /// 验证 EmitBatch 公共 API 可正常使用
    #[test]
    fn test_emit_batch_public_api() {
        let mut b: EmitBatch<i32> = EmitBatch::new();
        assert!(b.is_empty());

        b.push(0, 100);
        b.push(1, 200);
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());

        let drained = b.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], (0, 100));
        assert_eq!(drained[1], (1, 200));
        assert!(b.is_empty());
    }

    /// ASB 路径 1（≤4 槽）：验证单槽 emit 正确性
    #[test]
    fn test_asb_direct_path_single_slot() {
        let signal: Signal<i32> = Signal::named("asb_direct_single");
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        let _conn = signal
            .connect(move |v| {
                c.fetch_add(*v as usize, Ordering::SeqCst);
            })
            .unwrap();

        let result = signal.emit(&42);
        assert_eq!(result.invoked_count, 1);
        assert_eq!(result.total_count, 1);
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 42);
    }

    /// ASB 路径 1（≤4 槽）：验证 4 槽 emit 正确性与优先级顺序
    #[test]
    fn test_asb_direct_path_four_slots_with_priority() {
        let signal: Signal<()> = Signal::named("asb_direct_four");
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let o1 = Arc::clone(&order);
        let _c1 = signal
            .connect_with_priority(
                move |_| {
                    o1.lock().unwrap().push("low");
                },
                Priority::Low(0),
            )
            .unwrap();

        let o2 = Arc::clone(&order);
        let _c2 = signal
            .connect_with_priority(
                move |_| {
                    o2.lock().unwrap().push("normal");
                },
                Priority::Normal,
            )
            .unwrap();

        let o3 = Arc::clone(&order);
        let _c3 = signal
            .connect_with_priority(
                move |_| {
                    o3.lock().unwrap().push("high");
                },
                Priority::High(0),
            )
            .unwrap();

        let o4 = Arc::clone(&order);
        let _c4 = signal
            .connect(move |_| {
                o4.lock().unwrap().push("normal2");
            })
            .unwrap();

        let result = signal.emit(&());
        assert_eq!(result.invoked_count, 4);
        assert_eq!(result.total_count, 4);

        let order_guard = order.lock().unwrap();
        assert_eq!(*order_guard, vec!["high", "normal", "normal2", "low"]);
    }

    /// ASB 路径 2（5-64 槽）：验证中等数量槽位的 emit 正确性
    #[test]
    fn test_asb_snapshot_path_medium_slots() {
        let signal: Signal<i32> = Signal::named("asb_snapshot_medium");
        let counter = Arc::new(AtomicUsize::new(0));

        // 连接 10 个槽位（落入 5-64 槽路径）
        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let mut _conns: Vec<Connection<_>> = Vec::new();
        for _ in 0..10 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 10);
        assert_eq!(result.total_count, 10);
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    /// ASB 路径 3（>64 槽）：验证大数量槽位的 emit 正确性
    #[test]
    fn test_asb_batch_path_many_slots() {
        let signal: Signal<i32> = Signal::named("asb_batch_many");
        let counter = Arc::new(AtomicUsize::new(0));

        // 连接 100 个槽位（落入 >64 槽路径）
        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        const SLOT_COUNT: usize = 100;
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(SLOT_COUNT);
        for _ in 0..SLOT_COUNT {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, SLOT_COUNT);
        assert_eq!(result.total_count, SLOT_COUNT);
        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), SLOT_COUNT);
    }

    /// ASB 路径 3（>64 槽）：验证优先级顺序在大数量槽位下仍保持
    #[test]
    fn test_asb_batch_path_preserves_priority() {
        let signal: Signal<()> = Signal::named("asb_batch_priority");
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let mut _conns: Vec<Connection<_>> = Vec::new();

        // 70 个 Normal 槽
        for _ in 0..70 {
            let o = Arc::clone(&order);
            _conns.push(
                signal
                    .connect(move |_| {
                        o.lock().unwrap().push("normal");
                    })
                    .unwrap(),
            );
        }

        // 1 个 High 槽（应在最前）
        let o_high = Arc::clone(&order);
        _conns.push(
            signal
                .connect_with_priority(
                    move |_| {
                        o_high.lock().unwrap().push("high");
                    },
                    Priority::High(0),
                )
                .unwrap(),
        );

        // 1 个 Low 槽（应在最后）
        let o_low = Arc::clone(&order);
        _conns.push(
            signal
                .connect_with_priority(
                    move |_| {
                        o_low.lock().unwrap().push("low");
                    },
                    Priority::Low(0),
                )
                .unwrap(),
        );

        let result = signal.emit(&());
        assert_eq!(result.invoked_count, 72);

        let order_guard = order.lock().unwrap();
        assert_eq!(order_guard.len(), 72);
        // High 应在最前
        assert_eq!(order_guard[0], "high");
        // Low 应在最后
        assert_eq!(order_guard[71], "low");
    }

    /// ASB 路径 3（>64 槽）：验证 once 槽语义保持
    #[test]
    fn test_asb_batch_path_once_slot_semantics() {
        let signal: Signal<i32> = Signal::named("asb_batch_once");

        // 先连接 70 个普通槽（确保走 >64 路径）
        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let counter = Arc::new(AtomicUsize::new(0));
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(70);
        for _ in 0..70 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        // 连接 1 个 once 槽
        let c_once = Arc::clone(&counter);
        let conn = signal
            .connect_once(move |v| {
                c_once.fetch_add(*v as usize, Ordering::Relaxed);
            })
            .unwrap();

        assert_eq!(signal.slot_count(), 71);
        assert!(conn.is_connected());

        // 首次 emit：所有槽应执行
        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 71);
        assert_eq!(counter.load(Ordering::SeqCst), 71);

        // once 槽应已被移除
        assert_eq!(signal.slot_count(), 70);
        assert!(!conn.is_connected());

        // 二次 emit：只有 70 个普通槽执行
        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 70);
        assert_eq!(counter.load(Ordering::SeqCst), 141); // 71 + 70
    }

    /// ASB 路径 3（>64 槽）：验证错误隔离保持
    #[test]
    fn test_asb_batch_path_panic_isolation() {
        let signal: Signal<()> = Signal::named("asb_batch_panic");
        let counter = Arc::new(AtomicUsize::new(0));

        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let mut _conns: Vec<Connection<_>> = Vec::new();

        // 70 个普通槽
        for _ in 0..70 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |_| {
                        c.fetch_add(1, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        // 1 个 panic 槽
        _conns.push(
            signal
                .connect(|_| {
                    panic!("asb batch panic!");
                })
                .unwrap(),
        );

        // 再加 5 个普通槽（确保 panic 槽后还有槽）
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |_| {
                        c.fetch_add(1, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        let result = signal.emit(&());
        // 总槽位 76，1 个 panic，75 个成功
        assert_eq!(result.total_count, 76);
        assert_eq!(result.invoked_count, 75);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].message.contains("asb batch panic!"));
    }

    /// ASB 路径 3（>64 槽）：验证 ErrorPolicy::Stop 在批量路径下生效
    #[test]
    fn test_asb_batch_path_stop_on_error() {
        let signal: Signal<()> = Signal::named("asb_batch_stop");
        let counter = Arc::new(AtomicUsize::new(0));

        // 70 个普通槽
        for _ in 0..70 {
            let c = Arc::clone(&counter);
            signal
                .connect(move |_| {
                    c.fetch_add(1, Ordering::Relaxed);
                })
                .unwrap();
        }

        // 1 个 panic 槽（高优先级，确保最先执行）
        let _panic_conn = signal
            .connect_with_priority(
                |_| {
                    panic!("stop on error!");
                },
                Priority::High(0),
            )
            .unwrap();

        let result = signal.emit_with_options(&(), EmitOptions { on_error: ErrorPolicy::Stop });

        // High 优先级 panic 槽最先执行，失败后立即停止
        assert_eq!(result.invoked_count, 0);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// ASB：验证 disconnect_by_group 后 stats 同步（通过 emit 仍正常工作）
    #[test]
    fn test_asb_disconnect_by_group_keeps_consistency() {
        let signal: Signal<i32> = Signal::named("asb_disconnect_group");
        let counter = Arc::new(AtomicUsize::new(0));

        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        // 注意：debug 组的 Connection 在 disconnect_by_group 后仍保留在 _conns 中，
        // drop 时 CAS 失败（connected 已被 disconnect_by_group 设为 false），是 no-op。
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(80);

        // 连接 80 个槽，其中 30 个属于 "debug" 组
        for _ in 0..50 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }
        for _ in 0..30 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect_with_group(
                        move |v| {
                            c.fetch_add(*v as usize, Ordering::Relaxed);
                        },
                        "debug",
                    )
                    .unwrap(),
            );
        }

        assert_eq!(signal.slot_count(), 80);

        // 断开 debug 组
        signal.disconnect_by_group("debug");
        assert_eq!(signal.slot_count(), 50);

        // emit 仍正常工作（50 个槽）
        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 50);
        assert_eq!(result.total_count, 50);
        assert_eq!(counter.load(Ordering::SeqCst), 50);
    }

    /// ASB：验证多次 emit 后 SlotStats 仍能正常更新（>64 槽路径）
    #[test]
    fn test_asb_stats_update_after_multiple_emits() {
        let signal: Signal<i32> = Signal::named("asb_stats_multi");

        // 连接 70 个槽（>64 路径会更新 stats）
        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        let counter = Arc::new(AtomicUsize::new(0));
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(70);
        for _ in 0..70 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        // 多次 emit
        for _ in 0..1500 {
            let result = signal.emit(&1);
            assert_eq!(result.invoked_count, 70);
        }

        // 验证 counter 正确（70 * 1500 = 105000）
        assert_eq!(counter.load(Ordering::SeqCst), 70 * 1500);

        // 触发了至少一次 decay（每 1000 次 emit 触发一次）
        // 这里不直接验证 stats 内容（私有字段），只验证功能正确
    }

    /// ASB：验证 disconnect_all 后 stats 清空，emit 正常
    #[test]
    fn test_asb_disconnect_all_clears_stats() {
        let signal: Signal<i32> = Signal::named("asb_disconnect_all");
        let counter = Arc::new(AtomicUsize::new(0));

        // 保留 Connection 句柄，避免 Drop 时自动断开（P2 BUG-connection-no-drop 修复）
        // 注意：disconnect_all 后 Connection 仍保留在 _conns 中，
        // drop 时 CAS 失败（connected 已被 disconnect_all 设为 false），是 no-op。
        let mut _conns: Vec<Connection<_>> = Vec::with_capacity(100);
        for _ in 0..100 {
            let c = Arc::clone(&counter);
            _conns.push(
                signal
                    .connect(move |v| {
                        c.fetch_add(*v as usize, Ordering::Relaxed);
                    })
                    .unwrap(),
            );
        }

        assert_eq!(signal.slot_count(), 100);

        // emit 一次（走 >64 路径，更新 stats）
        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 100);
        assert_eq!(counter.load(Ordering::SeqCst), 100);

        // disconnect_all
        signal.disconnect_all();
        assert_eq!(signal.slot_count(), 0);
        assert!(signal.is_empty());

        // emit 后应无槽执行
        let result = signal.emit(&1);
        assert_eq!(result.invoked_count, 0);
        assert_eq!(result.total_count, 0);
        // counter 不变
        assert_eq!(counter.load(Ordering::SeqCst), 100);
    }
}
