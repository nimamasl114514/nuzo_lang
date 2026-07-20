//! # VM 核心单元测试套件
//!
//! 本模块包含 **Nuzo VM 的全面单元测试**，覆盖所有核心子系统：
//!
//! ## 测试分组
//!
//! ### 1. VM 初始化与生命周期
//! - `test_vm_new_creates_valid_vm`: 验证 `VM::new()` 创建的实例状态正确
//! - `test_vm_default_is_same_as_new`: 确保 `Default` trait 实现一致性
//! - `test_vm_init_gc_uses_custom_gc`: 验证自定义 GC 配置生效
//!
//! ### 2. 栈操作（Stack Operations）
//! - Push/Pop 的 LIFO 语义
//! - 栈溢出保护（`DEFAULT_MAX_STACK_SIZE` 检查）|
//! - 栈下溢（underflow）panic 触发
//! - Peek 操作不修改栈状态
//!
//! ### 3. 作用域与帧管理（Scoping & Frames）
//! - `push_scope` / `pop_scope` 的嵌套平衡性
//! - 局部变量的作用域隔离
//! - 帧换页机制（`FramePager`）|
//! - 跨作用域变量访问规则
//!
//! ### 4. 全局变量系统（Global Variables）
//! - `get_global` / `set_global` 的 CRUD 操作
//! - 未定义变量的错误处理
//! - 全局变量的持久性（跨多次执行）|
//!
//! ### 5. 错误处理与恢复（Error Handling）
//! - 运行时错误的捕获与传播
//! - 错误后的 VM 状态一致性（不残留脏数据）|
//! - 嵌套错误的正确处理
//!
//! ### 6. 边界条件（Edge Cases）
//! - 空源码执行
//! - 超长表达式求值
//! - 深层递归调用
//! - 大量全局变量压力测试
//!
//! ## 测试原则
//!
//! - **确定性**：所有测试无随机性，可重复执行
//! - **隔离性**：每个测试用独立的 VM 实例，互不干扰
//! - **完整性**：正常路径 + 边界条件 + 错误路径全覆盖
//! - **快速执行**：单个测试 < 10ms，整个套件 < 5 秒
//!
//! ## 与其他模块的关系
//!
//! 本测试模块是 [`nuzo_testkit`] 的**底层消费者**：
//! - [`bytecode_assert`] 宏内部会调用类似的断言逻辑
//! - [`e2e_runner`] 依赖本模块验证的基本功能正确性
//! - [`tracer`] / [`inspector`] 用于调试本模块中的失败用例

// VM Unit Tests (extracted from vm.rs for better organization)
#[cfg(test)]
#[allow(clippy::module_inception)] // 测试模块命名约定：vm_tests::tests，与被测模块对应
mod tests {
    use crate::gc::Gc;
    use crate::is_scratch;
    use crate::vm::VM;
    use nuzo_bytecode::{Chunk, Opcode};
    use nuzo_core::Value;
    use nuzo_values::{
        FALSE, HeapObject, InternalError, NIL, NuzoError, NuzoErrorKind, TRUE, ValueExt,
    };

    // =========================================================================
    // Test VM Initialization
    // =========================================================================

    #[test]
    fn test_vm_new_creates_valid_vm() {
        let vm = VM::new();

        assert!(!vm.is_running());
        assert_eq!(vm.stack_size(), 0);
        assert_eq!(vm.call_depth(), 0);
        assert!(vm.global_count() > 0);
    }

    #[test]
    fn test_vm_default_is_same_as_new() {
        let vm_new = VM::new();
        let vm_default = VM::default();

        assert_eq!(vm_new.is_running(), vm_default.is_running());
        assert_eq!(vm_new.stack_size(), vm_default.stack_size());
        assert_eq!(vm_new.call_depth(), vm_default.call_depth());
    }

    #[test]
    fn test_vm_init_gc_uses_custom_gc() {
        let custom_gc = Gc::new(2048);
        let vm = VM::init_gc(custom_gc);

        assert_eq!(vm.gc().threshold(), 2048);
    }

    #[test]
    fn test_allocate_box_uses_gc_heap_in_vm() {
        let _vm = VM::new();
        let idx = nuzo_values::value::allocate_box(nuzo_core::Value::from_number(42.0)).unwrap();
        assert!(
            idx >= nuzo_values::constants::HEAP_POOL_INDEX_LIMIT,
            "box should be allocated on GC heap, got idx {}",
            idx
        );
        assert_eq!(nuzo_values::value::get_box(idx), Some(nuzo_core::Value::from_number(42.0)));
        nuzo_values::value::set_box(idx, nuzo_core::Value::from_number(99.0)).unwrap();
        assert_eq!(nuzo_values::value::get_box(idx), Some(nuzo_core::Value::from_number(99.0)));
    }

    // =========================================================================
    // Test Stack Operations
    // =========================================================================

    #[test]
    fn test_two_vms_same_thread_gc_roots_not_crossed() {
        // VM1: create a heap object and keep it as a global root.
        let mut vm1 = VM::new();
        let mut halt_chunk = Chunk::new();
        halt_chunk.write_opcode(Opcode::Halt);
        vm1.run(halt_chunk).unwrap();

        let idx = vm1.gc_mut().alloc(HeapObject::Array(vec![Value::from_number(42.0)]));
        vm1.define_global("root", Value::from_gc_index(idx));
        assert!(vm1.gc().try_get(idx).is_some());

        // VM2: run on the same thread. With the old thread-local VM pointer,
        // this would overwrite the pointer used by VM1's GC roots callback.
        let mut vm2 = VM::new();
        let mut chunk2 = Chunk::new();
        chunk2.write_opcode(Opcode::Halt);
        vm2.run(chunk2).unwrap();

        // Trigger GC on VM1. The roots callback must use VM1's own pointer,
        // not a stale thread-local value, otherwise the root is unmarked and swept.
        vm1.gc_mut().collect();
        assert!(
            vm1.gc().try_get(idx).is_some(),
            "VM1 root object was collected due to crossed GC roots"
        );
    }

    #[test]
    fn test_push_pop_maintains_balance() {
        let mut vm = VM::new();

        // Push three values
        vm.push(Value::from_number(1.0)).unwrap();
        vm.push(Value::from_number(2.0)).unwrap();
        vm.push(Value::from_number(3.0)).unwrap();

        assert_eq!(vm.stack_size(), 3);

        // Pop them in LIFO order
        let v3 = vm.pop().unwrap();
        let v2 = vm.pop().unwrap();
        let v1 = vm.pop().unwrap();

        assert_eq!(v3.as_number(), 3.0);
        assert_eq!(v2.as_number(), 2.0);
        assert_eq!(v1.as_number(), 1.0);
        assert_eq!(vm.stack_size(), 0);
    }

    #[test]
    fn test_peek_does_not_modify_stack() {
        let mut vm = VM::new();

        vm.push(Value::from_number(42.0)).unwrap();

        // Peek multiple times
        let v1 = vm.peek(0).unwrap();
        let v2 = vm.peek(0).unwrap();
        let v3 = vm.peek(0).unwrap();

        assert_eq!(v1, v2);
        assert_eq!(v2, v3);
        assert_eq!(vm.stack_size(), 1); // Stack unchanged
    }

    #[test]
    fn test_peek_at_different_offsets() {
        let mut vm = VM::new();

        vm.push(Value::from_number(10.0)).unwrap();
        vm.push(Value::from_number(20.0)).unwrap();
        vm.push(Value::from_number(30.0)).unwrap();

        assert_eq!(vm.peek(0).unwrap().as_number(), 30.0); // Top
        assert_eq!(vm.peek(1).unwrap().as_number(), 20.0); // Second
        assert_eq!(vm.peek(2).unwrap().as_number(), 10.0); // Third
    }

    #[test]
    fn test_pop_from_empty_stack_returns_error() {
        let mut vm = VM::new();

        let result = vm.pop();
        assert!(result.is_err(), "Popping empty stack should fail");
    }

    #[test]
    fn test_peek_empty_stack_returns_error() {
        let vm = VM::new();

        let result = vm.peek(0);
        assert!(result.is_err(), "Peeking empty stack should fail");
    }

    #[test]
    fn test_clear_stack_resets_state() {
        let mut vm = VM::new();

        vm.push(Value::from_number(1.0)).unwrap();
        vm.push(NIL).unwrap();
        vm.push(TRUE).unwrap();

        assert_eq!(vm.stack_size(), 3);

        vm.clear_stack();

        assert_eq!(vm.stack_size(), 0);
    }

    // =========================================================================
    // Test Register Operations
    // =========================================================================

    #[test]
    fn test_set_and_get_register() {
        let mut vm = VM::new();

        // Set various registers
        let _ = vm.set_register(0, Value::from_number(42.0));
        let _ = vm.set_register(5, TRUE);
        let _ = vm.set_register(10, NIL);

        // Retrieve them
        assert_eq!(vm.register(0).unwrap().as_number(), 42.0);
        assert_eq!(vm.register(5).unwrap(), TRUE);
        assert_eq!(vm.register(10).unwrap(), NIL);
    }

    #[test]
    fn test_registers_auto_grow() {
        let mut vm = VM::new();

        // Set a high register index (forces growth)
        let _ = vm.set_register(200, Value::from_number(99.0));

        assert_eq!(vm.register(200).unwrap().as_number(), 99.0);

        // Intermediate registers should be default values
        assert_eq!(vm.register(0).unwrap(), Value::default());
    }

    #[test]
    fn test_get_out_of_bounds_register_returns_default() {
        let vm = VM::new();

        // 尝试访问不存在的寄存器
        let result = vm.register(50);

        // 修改后: 应该返回错误而非默认值
        assert!(result.is_err(), "Expected error for out-of-bounds register, got {:?}", result);

        if let Err(ref err) = result {
            if let NuzoErrorKind::Internal(
                InternalError::RegisterOutOfBounds { reg, available },
                _,
            ) = err.kind
            {
                assert_eq!(reg, 50);
                println!(
                    "✓ Correctly detected register {} out of bounds (available: {})",
                    reg, available
                );
            } else {
                panic!("Expected RegisterOutOfBounds error");
            }
        } else {
            panic!("Expected error for out-of-bounds register");
        }
    }

    // =========================================================================
    // Test Frame Management
    // =========================================================================

    #[test]
    fn test_push_and_pop_frame() {
        let mut vm = VM::new();

        // Push some values (simulating arguments)
        vm.push(Value::from_number(1.0)).unwrap();
        vm.push(Value::from_number(2.0)).unwrap();

        // Push frame (base should be at start of args)
        // push_frame 会将参数从调用者空间 [base-argc..base] 拷贝到被调用者空间 [base..base+argc]
        vm.push_frame(None, 2).unwrap();
        assert_eq!(vm.call_depth(), 1);

        // Pop frame - 返回值由调用者在 Return 指令中处理，pop_frame 只恢复栈状态
        vm.pop_frame().unwrap();
        assert_eq!(vm.call_depth(), 0);
    }

    #[test]
    fn test_nested_frames() {
        let mut vm = VM::new();

        // Push outer frame
        vm.push_frame(None, 0).unwrap();
        assert_eq!(vm.call_depth(), 1);

        // Push inner frame
        vm.push_frame(None, 0).unwrap();
        assert_eq!(vm.call_depth(), 2);

        // Pop inner frame
        vm.pop_frame().unwrap();
        assert_eq!(vm.call_depth(), 1);

        // Pop outer frame
        vm.pop_frame().unwrap();
        assert_eq!(vm.call_depth(), 0);
    }

    #[test]
    fn test_frame_paging_no_hard_limit() {
        let mut vm = VM::new();

        // 帧换页机制下，不再有硬限制
        // 超过 FRAME_PAGING_CAPACITY 的帧会被自动换出到堆上
        // push_frame 应始终成功
        let depth = crate::frame_paging::FRAME_PAGING_CAPACITY + 50;
        for _ in 0..depth {
            vm.push_frame(None, 0).unwrap();
        }

        // call_depth 应反映真实深度（含换出的帧）
        assert_eq!(vm.call_depth(), depth);
        // 帧换页统计应记录了换出操作
        let stats = vm.frame_pager_stats();
        assert!(stats.spill_count > 0, "Should have spilled frames when depth exceeds capacity");
    }

    #[test]
    fn test_pop_frame_from_empty_errors() {
        let mut vm = VM::new();

        let result = vm.pop_frame();
        assert!(result.is_err(), "Popping from empty frame stack should fail");
    }

    // =========================================================================
    // Test Global Variables
    // =========================================================================

    #[test]
    fn test_global_variable_operations() {
        let mut vm = VM::new();

        let builtin_count = vm.global_count();

        let idx1 = vm.add_global(Value::from_number(10.0));
        let idx2 = vm.add_global(TRUE);
        let idx3 = vm.add_global(NIL);

        assert_eq!(idx1, builtin_count);
        assert_eq!(idx2, builtin_count + 1);
        assert_eq!(idx3, builtin_count + 2);
        assert_eq!(vm.global_count(), builtin_count + 3);

        assert_eq!(vm.get_global(builtin_count).unwrap().as_number(), 10.0);
        assert_eq!(vm.get_global(builtin_count + 1).unwrap(), TRUE);
        assert_eq!(vm.get_global(builtin_count + 2).unwrap(), NIL);
    }

    #[test]
    fn test_set_global_updates_value() {
        let mut vm = VM::new();

        let idx = vm.add_global(Value::from_number(1.0));
        vm.set_global(idx, Value::from_number(99.0)).unwrap();

        assert_eq!(vm.get_global(idx).unwrap().as_number(), 99.0);
    }

    #[test]
    fn test_set_global_auto_expands() {
        let mut vm = VM::new();
        let initial_count = vm.global_count();

        // Set global at index 10 beyond current range (auto-expand)
        let target_idx = initial_count + 10;
        vm.set_global(target_idx, Value::from_number(42.0)).unwrap();

        assert_eq!(vm.global_count(), target_idx + 1);
        assert_eq!(vm.get_global(target_idx).unwrap().as_number(), 42.0);
    }

    // =========================================================================
    // Test Simple Program Execution
    // =========================================================================

    #[test]
    fn test_simple_halt_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk).unwrap();
        assert_eq!(result, NIL); // Nothing on stack, returns NIL
    }

    #[test]
    fn test_load_constant_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c = chunk.add_constant(Value::from_number(42.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0); // dest r0
        chunk.write_u16(c as u16);

        chunk.write_opcode(Opcode::Halt);

        let _result = vm.run(chunk).unwrap();

        // Result should be whatever's on top of stack
        // We pushed to r0, so we need to check register
        assert_eq!(vm.register(0).unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_load_literals_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Load nil into r0
        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(0);

        // Load true into r1
        chunk.write_opcode(Opcode::LoadTrue);
        chunk.write_u16(1);

        // Load false into r2
        chunk.write_opcode(Opcode::LoadFalse);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(0).unwrap(), NIL);
        assert_eq!(vm.register(1).unwrap(), TRUE);
        assert_eq!(vm.register(2).unwrap(), FALSE);
    }

    #[test]
    fn test_move_instruction() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Load 42 into r0
        let c = chunk.add_constant(Value::from_number(42.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c as u16);

        // Move r0 to r5
        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(5); // dest
        chunk.write_u16(0); // src

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(5).unwrap().as_number(), 42.0);
    }

    // =========================================================================
    // Test Arithmetic Operations
    // =========================================================================

    #[test]
    fn test_addition_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(32.0));

        // LoadK r0, 10
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        // LoadK r1, 32
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        // Add r2, r0, r1
        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2); // dest
        chunk.write_u16(0); // left
        chunk.write_u16(1); // right

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_subtraction_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(50.0));
        let c2 = chunk.add_constant(Value::from_number(8.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Sub);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_multiplication_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(6.0));
        let c2 = chunk.add_constant(Value::from_number(7.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Mul);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_division_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(84.0));
        let c2 = chunk.add_constant(Value::from_number(2.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Div);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap().as_number(), 42.0);
    }

    #[test]
    fn test_remainder_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(43.0));
        let c2 = chunk.add_constant(Value::from_number(10.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Rem);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap().as_number(), 3.0);
    }

    #[test]
    fn test_negation_program() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c = chunk.add_constant(Value::from_number(42.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c as u16);

        chunk.write_opcode(Opcode::Neg);
        chunk.write_u16(1); // dest
        chunk.write_u16(0); // src

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(1).unwrap().as_number(), -42.0);
    }

    #[test]
    fn test_division_by_zero_returns_error() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(0.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Div);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_err(), "Division by zero should return error");

        match result.unwrap_err().kind {
            NuzoErrorKind::DivisionByZero => {} // Expected
            other => panic!("Wrong error type: {:?}", other),
        }
    }

    #[test]
    fn test_type_error_in_arithmetic() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c_num = chunk.add_constant(Value::from_number(1.0));
        // Note: We can't add nil directly to constants easily in this context,
        // but we can load nil via LoadNil then try to add

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c_num as u16);

        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1); // Try to add number + nil

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_err(), "Adding number + nil should fail");
    }

    // =========================================================================
    // Test Comparison Operations
    // =========================================================================

    #[test]
    fn test_equality_comparison() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(10.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Eq);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap(), TRUE);
    }

    #[test]
    fn test_inequality_comparison() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(20.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Neq);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap(), TRUE);
    }

    #[test]
    fn test_less_than_comparison() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(5.0));
        let c2 = chunk.add_constant(Value::from_number(10.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Lt);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap(), TRUE);
    }

    #[test]
    fn test_greater_than_comparison() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(15.0));
        let c2 = chunk.add_constant(Value::from_number(10.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Gt);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(2).unwrap(), TRUE);
    }

    #[test]
    fn test_logical_not() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // NOT true = false
        chunk.write_opcode(Opcode::LoadTrue);
        chunk.write_u16(0);

        chunk.write_opcode(Opcode::Not);
        chunk.write_u16(1);
        chunk.write_u16(0);

        // NOT false = true
        chunk.write_opcode(Opcode::LoadFalse);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::Not);
        chunk.write_u16(3);
        chunk.write_u16(2);

        // NOT nil = true
        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Not);
        chunk.write_u16(5);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(1).unwrap(), FALSE); // NOT true
        assert_eq!(vm.register(3).unwrap(), TRUE); // NOT false
        assert_eq!(vm.register(5).unwrap(), TRUE); // NOT nil
    }

    // =========================================================================
    // Test Control Flow
    // =========================================================================

    #[test]
    fn test_unconditional_jump_forward() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Load 999 into r0
        let c1 = chunk.add_constant(Value::from_number(999.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        // Jump over the next instruction (which would set r0 to 111)
        // Jmp offset needs to skip: LoadK(5 bytes) + Halt(1 byte) = 6 bytes
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(6); // Skip forward 6 bytes

        // This should be skipped
        let c2 = chunk.add_constant(Value::from_number(111.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(0).unwrap().as_number(), 999.0);
    }

    // =========================================================================
    // Test GetIndex and SetIndex Operations (Array & Dict)
    // =========================================================================

    #[test]
    fn test_array_get_index() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c10 = chunk.add_constant(Value::from_number(10.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c10 as u16);

        let c20 = chunk.add_constant(Value::from_number(20.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(2);
        chunk.write_u16(c20 as u16);

        let c30 = chunk.add_constant(Value::from_number(30.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(3);
        chunk.write_u16(c30 as u16);

        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(0);
        chunk.write_u16(3);

        let c1 = chunk.add_constant(Value::from_number(1.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(4);
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::GetIndex);
        chunk.write_u16(5);
        chunk.write_u16(0);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        let result = vm.register(5).unwrap();
        assert_eq!(result.as_number(), 20.0);
    }

    #[test]
    fn test_array_set_index() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c10 = chunk.add_constant(Value::from_number(10.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c10 as u16);

        let c20 = chunk.add_constant(Value::from_number(20.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(2);
        chunk.write_u16(c20 as u16);

        let c30 = chunk.add_constant(Value::from_number(30.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(3);
        chunk.write_u16(c30 as u16);

        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(0);
        chunk.write_u16(3);

        let c0 = chunk.add_constant(Value::from_number(0.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(4);
        chunk.write_u16(c0 as u16);

        let c99 = chunk.add_constant(Value::from_number(99.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(5);
        chunk.write_u16(c99 as u16);

        chunk.write_opcode(Opcode::SetIndex);
        chunk.write_u16(0);
        chunk.write_u16(4);
        chunk.write_u16(5);

        chunk.write_opcode(Opcode::GetIndex);
        chunk.write_u16(6);
        chunk.write_u16(0);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        let result = vm.register(6).unwrap();
        assert_eq!(result.as_number(), 99.0);
    }

    #[test]
    fn test_array_set_index_auto_expand() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c10 = chunk.add_constant(Value::from_number(10.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c10 as u16);

        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(0);
        chunk.write_u16(1);

        let c5 = chunk.add_constant(Value::from_number(5.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(2);
        chunk.write_u16(c5 as u16);

        let c999 = chunk.add_constant(Value::from_number(999.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(3);
        chunk.write_u16(c999 as u16);

        chunk.write_opcode(Opcode::SetIndex);
        chunk.write_u16(0);
        chunk.write_u16(2);
        chunk.write_u16(3);

        chunk.write_opcode(Opcode::GetIndex);
        chunk.write_u16(4);
        chunk.write_u16(0);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        let result = vm.register(4).unwrap();
        assert_eq!(result.as_number(), 999.0);
    }

    #[test]
    fn test_array_new_supports_empty_then_setindex_encoding() {
        use nuzo_values::HeapObject;

        let mut vm = VM::new();
        let mut chunk = Chunk::new();
        chunk.locals_count = 257;

        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(256);
        chunk.write_u16(256);

        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(0);
        chunk.write_u16(256);

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk).unwrap();
        let obj = result.as_heap_object_opt().expect("ArrayNew should produce a heap object");
        match obj.as_ref() {
            HeapObject::Array(items) => {
                assert_eq!(items.len(), 256, "empty-array encoding should pre-size the array");
                assert!(
                    items.iter().all(|v| v.is_nil()),
                    "pre-sized slots should be initialized to nil"
                );
            }
            other => panic!("expected array heap object, got {:?}", other),
        }
    }

    #[test]
    fn test_dict_get_and_set_index() {
        use nuzo_values::{HeapObject, NuzoDict};
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Create empty Dict via constant pool + LoadK (DictNew removed)
        let dict_val = Value::from_heap_object_gc(HeapObject::Dict(NuzoDict::new()));
        let dict_idx = chunk.add_constant(dict_val);
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(dict_idx as u16);

        let cname = chunk.add_constant(Value::from_string("name"));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(cname as u16);

        let calice = chunk.add_constant(Value::from_string("Alice"));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(2);
        chunk.write_u16(calice as u16);

        chunk.write_opcode(Opcode::SetIndex);
        chunk.write_u16(0);
        chunk.write_u16(1);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::GetIndex);
        chunk.write_u16(3);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        let result = vm.register(3).unwrap();
        assert_eq!(result.as_string_opt().unwrap(), "Alice");
    }

    /// R6 regression: property-access PIC must use a stable shape ID, not just
    /// object length. Two dicts with the same length but different property
    /// names must not produce a false PIC hit.
    #[test]
    fn test_pic_shape_id_no_false_hit_for_same_length_different_keys() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        // The loop body executes the same GetProp instruction against two
        // different dicts: {a: 1} and {b: 2}. With a length-only guard the
        // second iteration would falsely use the cached slot from the first
        // dict and return 2.0; with a real shape ID it correctly returns nil.
        let source = r#"
d1 = {a: 1}
d2 = {b: 2}
arr = [d1, d2]
i = 0
while i < 2 {
    x = arr[i].a
    i = i + 1
}
"#;
        let chunk = Compiler::compile(source).expect("compile R6 PIC regression test");
        let result = vm.run(chunk);
        assert!(result.is_ok(), "R6 PIC regression should run: {:?}", result.err());

        let x = vm.get_global_by_name("x").expect("global x should exist");
        assert!(
            x.is_nil(),
            "second dict has no property 'a'; false PIC hit would return 2.0, got {:?}",
            x
        );
    }

    #[test]
    fn test_array_index_out_of_bounds_returns_error() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(1.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c1 as u16);

        let c2 = chunk.add_constant(Value::from_number(2.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c2 as u16);

        let c3 = chunk.add_constant(Value::from_number(3.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(2);
        chunk.write_u16(c3 as u16);

        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(0);
        chunk.write_u16(3);

        let c100 = chunk.add_constant(Value::from_number(100.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c100 as u16);

        chunk.write_opcode(Opcode::GetIndex);
        chunk.write_u16(3);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("out of bounds"), "Expected index out of bounds error, got: {}", err);
    }

    #[test]
    fn test_conditional_jump_with_truthy_value() -> Result<(), NuzoError> {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let false_idx = chunk.add_constant(Value::from_bool(false));
        let true_idx = chunk.add_constant(Value::from_bool(true));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(false_idx as u16);

        chunk.write_opcode(Opcode::Test);
        chunk.write_u16(0);
        chunk.write_i16(5);

        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(0);
        chunk.write_u16(0);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(true_idx as u16);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk)?;

        assert!(vm.register(0)?.as_bool());

        Ok(())
    }

    // =========================================================================
    // Test Print Operation
    // =========================================================================

    #[test]
    fn test_print_operation() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c = chunk.add_constant(Value::from_number(42.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c as u16);

        chunk.write_opcode(Opcode::Print);
        chunk.write_u16(0);

        chunk.write_opcode(Opcode::Halt);

        // Capture output (would print "42\n")
        vm.run(chunk).unwrap();

        // If we got here, print didn't crash
        assert_eq!(vm.register(0).unwrap().as_number(), 42.0);
    }

    // =========================================================================
    // Test Complex Programs
    // =========================================================================

    #[test]
    fn test_complex_arithmetic_expression() {
        // Compute: (10 + 20) * 3 - 5 = 85
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c10 = chunk.add_constant(Value::from_number(10.0));
        let c20 = chunk.add_constant(Value::from_number(20.0));
        let c3 = chunk.add_constant(Value::from_number(3.0));
        let c5 = chunk.add_constant(Value::from_number(5.0));

        // r0 = 10
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c10 as u16);

        // r1 = 20
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c20 as u16);

        // r2 = r0 + r1 = 30
        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        // r3 = 3
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(3);
        chunk.write_u16(c3 as u16);

        // r4 = r2 * r3 = 90
        chunk.write_opcode(Opcode::Mul);
        chunk.write_u16(4);
        chunk.write_u16(2);
        chunk.write_u16(3);

        // r5 = 5
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(5);
        chunk.write_u16(c5 as u16);

        // r6 = r4 - r5 = 85
        chunk.write_opcode(Opcode::Sub);
        chunk.write_u16(6);
        chunk.write_u16(4);
        chunk.write_u16(5);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(6).unwrap().as_number(), 85.0);
    }

    #[test]
    fn test_multiple_operations_sequentially() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let vals = [1.0, 2.0, 3.0, 4.0, 5.0];
        let mut indices = Vec::new();

        // Load all values
        for (i, &val) in vals.iter().enumerate() {
            let idx = chunk.add_constant(Value::from_number(val));
            indices.push(idx);

            chunk.write_opcode(Opcode::LoadK);
            chunk.write_u16(i as u16);
            chunk.write_u16(idx as u16);
        }

        // Chain additions: r5 = ((1+2)+3)+4)+5 = 15
        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(5);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(6);
        chunk.write_u16(5);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(7);
        chunk.write_u16(6);
        chunk.write_u16(3);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(8);
        chunk.write_u16(7);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(8).unwrap().as_number(), 15.0);
    }

    // =========================================================================
    // Integration Test (from requirements)
    // =========================================================================

    #[test]
    fn test_integration_compute_42_print_halt() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c1 = chunk.add_constant(Value::from_number(10.0));
        let c2 = chunk.add_constant(Value::from_number(32.0));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0); // r0
        chunk.write_u16(c1 as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1); // r1
        chunk.write_u16(c2 as u16);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2); // r2 = result
        chunk.write_u16(0); // left
        chunk.write_u16(1); // right

        chunk.write_opcode(Opcode::Print);
        chunk.write_u16(2); // print r2

        // Move result to r0 (convention: VM returns r0)
        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(0); // dest: r0
        chunk.write_u16(2); // src: r2

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk).unwrap();

        assert!(result.as_number() == 42.0, "Result should be 42.0, got {:?}", result);
    }

    // =========================================================================
    // Error Recovery Tests
    // =========================================================================

    #[test]
    fn test_vm_recovers_after_error() {
        let mut vm = VM::new();

        // First run: cause an error (division by zero)
        let mut bad_chunk = Chunk::new();
        let c1 = bad_chunk.add_constant(Value::from_number(1.0));
        let c2 = bad_chunk.add_constant(Value::from_number(0.0));

        bad_chunk.write_opcode(Opcode::LoadK);
        bad_chunk.write_u16(0);
        bad_chunk.write_u16(c1 as u16);

        bad_chunk.write_opcode(Opcode::LoadK);
        bad_chunk.write_u16(1);
        bad_chunk.write_u16(c2 as u16);

        bad_chunk.write_opcode(Opcode::Div);
        bad_chunk.write_u16(2);
        bad_chunk.write_u16(0);
        bad_chunk.write_u16(1);

        bad_chunk.write_opcode(Opcode::Halt);

        let result1 = vm.run(bad_chunk);
        assert!(result1.is_err());

        // VM should still be usable for another run
        let mut good_chunk = Chunk::new();
        let c = good_chunk.add_constant(Value::from_number(99.0));

        good_chunk.write_opcode(Opcode::LoadK);
        good_chunk.write_u16(0);
        good_chunk.write_u16(c as u16);

        good_chunk.write_opcode(Opcode::Halt);

        let result2 = vm.run(good_chunk);
        assert!(result2.is_ok(), "VM should recover and run successfully");

        assert_eq!(vm.register(0).unwrap().as_number(), 99.0);
    }

    #[test]
    fn test_invalid_opcode_returns_error() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Byte 41 is outside the valid Opcode discriminant range (0..=40),
        // so decode_opcode returns None and fetch_opcode yields InvalidOpcode.
        chunk.code_mut().push(41);
        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_err(), "Invalid opcode should return error");
    }

    #[test]
    fn test_out_of_bounds_constant_returns_error() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Try to load constant at index 999 (doesn't exist)
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(999); // Out of bounds

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_err(), "Out-of-bounds constant should fail");
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_empty_chunk_runs_successfully() {
        let mut vm = VM::new();
        let chunk = Chunk::new(); // Empty chunk

        // Should immediately exit (no instructions to execute)
        let result = vm.run(chunk);
        assert!(result.is_ok(), "Empty chunk should run successfully");
        assert_eq!(result.unwrap(), NIL);
    }

    #[test]
    fn test_large_register_indices() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c = chunk.add_constant(Value::from_number(255.0));

        // Use high register indices
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(250); // High register index
        chunk.write_u16(c as u16);

        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(255); // Max u8 register
        chunk.write_u16(250);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(255).unwrap().as_number(), 255.0);
    }

    #[test]
    fn test_many_constants() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Add many constants
        let num_consts = 100;
        for i in 0..num_consts {
            chunk.add_constant(Value::from_number(i as f64));
        }

        // Load last constant
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16((num_consts - 1) as u16);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        assert_eq!(vm.register(0).unwrap().as_number(), (num_consts - 1) as f64);
    }

    #[test]
    fn test_floating_point_precision() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let pi = chunk.add_constant(Value::from_number(std::f64::consts::PI));
        let e = chunk.add_constant(Value::from_number(std::f64::consts::E));

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(pi as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(e as u16);

        chunk.write_opcode(Opcode::Add);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        vm.run(chunk).unwrap();

        let result = vm.register(2).unwrap().as_number();
        let expected = std::f64::consts::PI + std::f64::consts::E;
        assert!((result - expected).abs() < f64::EPSILON, "PI + E should maintain precision");
    }

    // =========================================================================
    // Stress Tests
    // =========================================================================

    #[test]
    fn test_stress_many_instructions() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Emit 1000 LoadNil instructions
        for i in 0..1000u16 {
            chunk.write_opcode(Opcode::LoadNil);
            chunk.write_u16(i % 256); // Cycle through registers
        }

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_ok(), "Should handle 1000+ instructions");
    }

    #[test]
    fn test_stress_deep_call_frames() {
        let mut vm = VM::new();

        // Push many frames (but stay under limit)
        for _ in 0..500 {
            vm.push_frame(None, 0).unwrap();
        }

        assert_eq!(vm.call_depth(), 500);

        // Pop them all
        for _ in 0..500 {
            vm.pop_frame().unwrap();
        }

        assert_eq!(vm.call_depth(), 0);
    }

    // =========================================================================
    // Test All Opcodes Execute Without Crashing
    // =========================================================================

    #[test]
    fn test_all_opcodes_basic_execution() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // Prepare constants
        let c_num = chunk.add_constant(Value::from_number(42.0));

        // Constants & Variables
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c_num as u16);

        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::LoadTrue);
        chunk.write_u16(2);

        chunk.write_opcode(Opcode::LoadFalse);
        chunk.write_u16(3);

        chunk.write_opcode(Opcode::Mov);
        chunk.write_u16(4);
        chunk.write_u16(0);

        // Arithmetic
        chunk.write_opcode(Opcode::Neg);
        chunk.write_u16(5);
        chunk.write_u16(0);

        // Comparison
        chunk.write_opcode(Opcode::Eq);
        chunk.write_u16(6);
        chunk.write_u16(0);
        chunk.write_u16(0);

        chunk.write_opcode(Opcode::Not);
        chunk.write_u16(7);
        chunk.write_u16(2); // NOT true

        // Control flow
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(0); // Jump 0 (no-op)

        // Built-in
        chunk.write_opcode(Opcode::Print);
        chunk.write_u16(0);

        // Termination
        chunk.write_opcode(Opcode::Halt);

        // All opcodes above should execute without crashing (returning Ok or Err is fine)
        let result = vm.run(chunk);
        assert!(result.is_ok(), "All basic opcodes should execute successfully");
    }

    // =========================================================================
    // 🐛 BUG 验证测试：is_truthy() 对数字 0 的处理
    // =========================================================================

    /// 测试 Test 指令对数字 0 的行为
    ///
    /// **Bug 已修复** (2025-05-31, value.rs:is_truthy):
    /// `is_truthy()` 新增 `self.is_number() && self.as_number() == 0.0` 检查，
    /// 数字 0（含 -0.0）现在正确返回 falsy。
    ///
    /// 参考回归测试: `crates/nuzo_testkit/src/stress_test.rs`
    #[test]
    fn test_test_with_zero_value() {
        use nuzo_core::Value;

        // 直接测试 Value::is_truthy() 的行为
        let zero = Value::from_number(0.0);
        let neg_zero = Value::from_number(-0.0);
        let one = Value::from_number(1.0);
        let neg_one = Value::from_number(-1.0);
        let nil = nuzo_values::NIL;
        let false_val = nuzo_values::FALSE;
        let true_val = nuzo_values::TRUE;

        // ✅ 修复后：数字 0 正确返回 falsy
        assert!(!zero.is_truthy(), "数字 0 应该是 falsy");
        assert!(!neg_zero.is_truthy(), "负零 (-0.0) 应该是 falsy");
        assert!(one.is_truthy(), "正数应该是 truthy");
        assert!(neg_one.is_truthy(), "负数应该是 truthy");

        // ✅ 其他类型行为不变
        assert!(!nil.is_truthy(), "nil 应该是 falsy");
        assert!(!false_val.is_truthy(), "false 应该是 falsy");
        assert!(true_val.is_truthy(), "true 应该是 truthy");
    }

    /// 测试 While 循环在条件为 0 时的行为
    ///
    /// **Bug 已修复** (2025-05-31):
    /// `is_truthy()` 修复后，数字 0 为 falsy，while 循环在条件为 0 时立即退出。
    ///
    /// 字节码布局（1-based 行号对应地址）：
    ///   0-4:  LoadK  r0=0        (1 op + 2 reg + 2 const = 5 bytes)
    ///   5-9:  Test  r0, +15      (1 op + 2 reg + 2 offset = 5 bytes)
    ///  10-14: LoadK  r1=1       (5 bytes)
    ///  15-21: Sub   r0=r0-r1    (1 op + 2+2+2 regs = 7 bytes)
    ///  22-24: Jmp   -20         (1 op + 2 offset = 3 bytes)
    ///     25: Halt              (1 byte)
    ///
    /// ```nuzo
    /// x = 0
    /// while x {   // 立即退出（正确行为）
    ///     x = x - 1
    /// }
    /// ```
    #[test]
    fn test_while_loop_with_zero_condition_should_exit() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // r0 = 0 (初始值)
        let c_zero = chunk.add_constant(Value::from_number(0.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0); // dest: r0
        chunk.write_u16(c_zero as u16);

        // 循环开始 (地址 5)
        // Test r0, 如果 falsy 则跳转到循环结束 (Halt at address 25)
        // IP after reading args = 10, target = 25, offset = 15
        chunk.write_opcode(Opcode::Test);
        chunk.write_u16(0); // test register: r0
        chunk.write_i16(15); // offset: 跳过循环体到 Halt (25 - 10 = 15)

        // 循环体：r1 = 1 (地址 10)
        let c_one = chunk.add_constant(Value::from_number(1.0));
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1); // dest: r1
        chunk.write_u16(c_one as u16);

        // r0 = r0 - r1 (地址 15)
        chunk.write_opcode(Opcode::Sub);
        chunk.write_u16(0); // dest: r0
        chunk.write_u16(0); // left: r0
        chunk.write_u16(1); // right: r1

        // Jmp 回循环开始 (地址 22)
        // IP after reading args = 25, target = 5, offset = -20
        chunk.write_opcode(Opcode::Jmp);
        chunk.write_i16(-20); // 跳回 Test at address 5 (5 - 25 = -20)

        // 循环结束：Halt (地址 25)
        chunk.write_opcode(Opcode::Halt);

        // 执行字节码
        let result = vm.run(chunk);

        // ✅ Bug 修复后：循环应该立即退出，r0 保持为 0
        assert!(result.is_ok(), "While 循环应该正常退出，不会死循环或崩溃");

        let final_value = vm.register(0).expect("r0 should exist");
        assert_eq!(
            final_value.as_number(),
            0.0,
            "循环应该在 r0=0 时立即退出（不进入循环体），r0 保持为 0"
        );
    }

    /// 因果注入测试：验证 Test 与 Jmp 对越界跳转目标的处理一致性
    ///
    /// **调试方法**：因果注入测试（Causal Injection Testing）
    ///
    /// 核心假设：跳转指令的“越界目标”是导致 VM 行为不一致的根因。
    /// 我们向 Jmp 和 Test 注入相同的越界目标，观察两者是否都报错。
    /// 如果 Test 静默忽略而 Jmp 报错，就说明 Test 的越界检查存在 bug。
    #[test]
    fn test_causal_injection_test_and_jmp_out_of_bounds() {
        use nuzo_values::TRUE;

        // ================================================================
        // 对照组 A：Jmp 跳转到合法目标（应成功）
        // ================================================================
        {
            let mut vm = VM::new();
            let mut chunk = Chunk::new();
            let true_const = chunk.add_constant(TRUE);

            // 地址 0: LoadK r0, true_const
            chunk.write_opcode(Opcode::LoadK);
            chunk.write_u16(0);
            chunk.write_u16(true_const as u16);

            // 地址 5: Jmp +1 (跳过下一条 Halt 到达地址 9 的 Halt)
            // IP 读完 Jmp 操作数后 = 5 + 3 = 8, target = 8 + 1 = 9
            chunk.write_opcode(Opcode::Jmp);
            chunk.write_i16(1);

            // 地址 8: Halt（会被跳过）
            chunk.write_opcode(Opcode::Halt);

            // 地址 9: Halt（真正结束）
            chunk.write_opcode(Opcode::Halt);

            assert!(vm.run(chunk).is_ok(), "对照组 A：Jmp 到合法目标应成功");
        }

        // ================================================================
        // 因果注入 A：Jmp 注入越界目标（应失败）
        // ================================================================
        {
            let mut vm = VM::new();
            let mut chunk = Chunk::new();

            // 地址 0: Jmp +9999（明显越界）
            // IP 读完操作数后 = 3, target = 3 + 9999 = 10002
            chunk.write_opcode(Opcode::Jmp);
            chunk.write_i16(9999);

            chunk.write_opcode(Opcode::Halt);

            assert!(vm.run(chunk).is_err(), "因果注入 A：Jmp 注入越界目标应报错");
        }

        // ================================================================
        // 对照组 B：Test 跳转到合法目标（应成功）
        // ================================================================
        {
            let mut vm = VM::new();
            let mut chunk = Chunk::new();
            let false_const = chunk.add_constant(Value::from_bool(false));

            // 地址 0: LoadK r0, false
            chunk.write_opcode(Opcode::LoadK);
            chunk.write_u16(0);
            chunk.write_u16(false_const as u16);

            // 地址 5: Test r0, +1
            // IP 读完 Test 操作数后 = 5 + 5 = 10, target = 10 + 1 = 11
            chunk.write_opcode(Opcode::Test);
            chunk.write_u16(0);
            chunk.write_i16(1);

            // 地址 10: Halt（会被跳过，因为 r0 是 falsy）
            chunk.write_opcode(Opcode::Halt);

            // 地址 11: Halt（真正结束）
            chunk.write_opcode(Opcode::Halt);

            assert!(vm.run(chunk).is_ok(), "对照组 B：Test 到合法目标应成功");
        }

        // ================================================================
        // 因果注入 B：Test 注入越界目标（应失败）
        // ================================================================
        {
            let mut vm = VM::new();
            let mut chunk = Chunk::new();
            let false_const = chunk.add_constant(Value::from_bool(false));

            // 地址 0: LoadK r0, false
            chunk.write_opcode(Opcode::LoadK);
            chunk.write_u16(0);
            chunk.write_u16(false_const as u16);

            // 地址 5: Test r0, +9999（明显越界）
            // IP 读完操作数后 = 10, target = 10 + 9999 = 10009
            chunk.write_opcode(Opcode::Test);
            chunk.write_u16(0);
            chunk.write_i16(9999);

            chunk.write_opcode(Opcode::Halt);

            let result = vm.run(chunk);
            assert!(result.is_err(), "因果注入 B：Test 注入越界目标应报错；实际结果: {:?}", result);
        }
    }

    /// 测试 LT 指令对负数的处理
    ///
    /// 验证比较操作在边界情况下是否正确
    #[test]
    fn test_lt_with_negative_numbers() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        // 准备常量
        let c_zero = chunk.add_constant(Value::from_number(0.0));
        let c_neg_one = chunk.add_constant(Value::from_number(-1.0));
        let c_one = chunk.add_constant(Value::from_number(1.0));

        // 测试 1: 0 < -1 → false
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0); // r0 = 0
        chunk.write_u16(c_zero as u16);

        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1); // r1 = -1
        chunk.write_u16(c_neg_one as u16);

        chunk.write_opcode(Opcode::Lt);
        chunk.write_u16(2); // r2 = (r0 < r1) 即 (0 < -1)
        chunk.write_u16(0);
        chunk.write_u16(1);

        // 测试 2: -1 < 0 → true
        chunk.write_opcode(Opcode::Lt);
        chunk.write_u16(3); // r3 = (r1 < r0) 即 (-1 < 0)
        chunk.write_u16(1);
        chunk.write_u16(0);

        // 测试 3: 0 < 1 → true
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(4); // r4 = 1
        chunk.write_u16(c_one as u16);

        chunk.write_opcode(Opcode::Lt);
        chunk.write_u16(5); // r5 = (r0 < r4) 即 (0 < 1)
        chunk.write_u16(0);
        chunk.write_u16(4);

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_ok(), "LT 操作不应该出错");

        // 验证结果
        let r2 = vm.register(2).expect("r2 should exist");
        let r3 = vm.register(3).expect("r3 should exist");
        let r5 = vm.register(5).expect("r5 should exist");

        eprintln!("[TEST] 0 < -1 = {} (期望: false)", r2.as_bool());
        eprintln!("[TEST] -1 < 0 = {} (期望: true)", r3.as_bool());
        eprintln!("[TEST] 0 < 1 = {} (期望: true)", r5.as_bool());

        assert!(!r2.as_bool(), "0 < -1 应该是 false");
        assert!(r3.as_bool(), "-1 < 0 应该是 true");
        assert!(r5.as_bool(), "0 < 1 应该是 true");
    }

    /// 测试 SUB 指令的溢出处理
    ///
    /// 验证 0 - 1 是否正确返回 -1
    #[test]
    fn test_sub_zero_minus_one() {
        let mut vm = VM::new();
        let mut chunk = Chunk::new();

        let c_zero = chunk.add_constant(Value::from_number(0.0));
        let c_one = chunk.add_constant(Value::from_number(1.0));

        // r0 = 0
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(0);
        chunk.write_u16(c_zero as u16);

        // r1 = 1
        chunk.write_opcode(Opcode::LoadK);
        chunk.write_u16(1);
        chunk.write_u16(c_one as u16);

        // r2 = r0 - r1 (即 0 - 1)
        chunk.write_opcode(Opcode::Sub);
        chunk.write_u16(2);
        chunk.write_u16(0);
        chunk.write_u16(1);

        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(result.is_ok(), "SUB 操作不应该出错");

        let r2 = vm.register(2).expect("r2 should exist");
        eprintln!("[TEST] 0 - 1 = {} (期望: -1)", r2.as_number());

        assert_eq!(r2.as_number(), -1.0, "0 - 1 应该等于 -1");
    }

    // =========================================================================
    // ExecutionContext + Tracer 回归测试
    // =========================================================================
    //
    // 验证 ExecutionContext 隔离模型修复了 Tracer 环境下 builtin 函数
    //（如 println）被错误解析为 "object" 而非 function 的 bug。
    //
    // 根因：旧 VM 结构体的可变状态没有生命周期边界，chunk 切换时
    // hot_trace_table / call_sites / inline_cache 等瞬态状态残留导致污染。
    // ExecutionContext 的 snapshot_for_chunk_switch() 从架构上杜绝此问题。

    #[test]
    fn test_tracer_println_resolves_as_function() {
        use crate::tracer_state::TraceConfig;
        use nuzo_compiler::Compiler;

        let config = TraceConfig::default();
        let (mut vm, output_buf) = VM::new_with_output_capture_and_tracer(config);

        // 编译并执行 println(1) —— 这是之前 Tracer bug 的最小复现
        let source = "println(1);";
        let chunk = Compiler::compile(source).expect("Compilation should succeed");

        let result = vm.run(chunk);

        // 核心断言：不能有 TypeMismatch("expected function, actual object")
        assert!(
            result.is_ok(),
            "println(1) should succeed in tracer VM, got error: {:?}",
            result.err()
        );

        // 验证输出捕获正确
        let output = output_buf.lock().expect("Mutex not poisoned");
        assert!(
            output.iter().any(|line| line.contains('1')),
            "Output should contain '1', got: {:?}",
            *output
        );
    }

    #[test]
    fn test_execution_context_snapshot_clears_transient_state() {
        // 验证 snapshot_for_chunk_switch 正确清空所有瞬态状态
        let mut vm = VM::new();

        // 第一次执行：写入一些瞬态状态
        let source1 = "a = 42; b = a + 8;";
        let chunk1 = nuzo_compiler::Compiler::compile(source1).unwrap();
        let r1 = vm.run(chunk1);
        assert!(r1.is_ok(), "First run should succeed");

        // 第二次执行：验证瞬态状态已被清空（无残留污染）
        let source2 = "x = 100; y = x * 2;";
        let chunk2 = nuzo_compiler::Compiler::compile(source2).unwrap();
        let r2 = vm.run(chunk2);
        assert!(
            r2.is_ok(),
            "Second run should succeed (no transient state leakage), got: {:?}",
            r2.err()
        );

        // 验证全局作用域跨 execute 保留（global_scope 不被 snapshot 清空）
        let val_a = vm.lookup_global("a");
        assert!(val_a.is_some(), "Global 'a' from first execution should persist in GlobalScope");
    }

    #[test]
    fn test_tracer_multiple_executions_no_state_leak() {
        // 连续多次 execute，验证 Tracer 状态不泄漏
        use crate::tracer_state::TraceConfig;
        use nuzo_compiler::Compiler;

        let config = TraceConfig::default();
        let (mut vm, _output_buf) = VM::new_with_output_capture_and_tracer(config);

        for i in 1..=5i64 {
            let source = format!("println({});", i);
            let chunk = Compiler::compile(&source)
                .unwrap_or_else(|e| panic!("Compile failed for iteration {}: {:?}", i, e));
            let result = vm.run(chunk);
            assert!(
                result.is_ok(),
                "Iteration {}: println({}) should succeed in tracer VM, got: {:?}",
                i,
                i,
                result.err()
            );
        }
    }

    /// Regression test: B1-style while loop with many iterations should not stack overflow.
    ///
    /// Bug: After dispatch.rs update (CIGC/μPIC/Unrolled Builtin/TCO/Fast Capture),
    /// compiled while loops with high iteration counts caused stack overflow in debug mode
    /// (and 512GB OOM in release mode).
    #[test]
    fn test_b1_loop_no_stack_overflow() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        let source = r#"
            i = 0
            while i < 1000000 { i = i + 1 }
        "#;
        let chunk = Compiler::compile(source).expect("B1 compilation should succeed");
        let result = vm.run(chunk);
        assert!(
            result.is_ok(),
            "B1-style loop should complete without stack overflow, got: {:?}",
            result.err()
        );
    }

    /// Regression test: B2 arithmetic loop should complete (currently fails).
    #[test]
    fn test_b2_arithmetic_loop() {
        use nuzo_compiler::Compiler;

        // Dump bytecode for both passing and failing formats to find the difference
        let passing =
            "i = 0; result = 0; while i < 3 { result = result + i * 2 - 1 / 3; i = i + 1 }";
        let failing = "result = 0\ni = 0\nwhile i < 3 {\n    result = result + i * 2 - 1 / 3\n    i = i + 1\n}";

        for (label, source) in [("PASSING", passing), ("FAILING", failing)] {
            match Compiler::compile(source) {
                Ok(chunk) => {
                    eprintln!("=== {} BYTECODE ===", label);
                    eprintln!("code_len={} locals={}", chunk.code().len(), chunk.locals_count);
                    eprintln!("constants: {:?}", chunk.constants());
                    // Disassemble
                    let mut ip = 0;
                    while ip < chunk.code().len() {
                        if let Some(op) = nuzo_bytecode::Chunk::decode_opcode(chunk.code()[ip]) {
                            let size = op.instruction_size();
                            let bytes: Vec<u8> =
                                chunk.code()[ip..ip + size.min(chunk.code().len() - ip)].to_vec();
                            eprintln!("  [{:04}] {:?} {:?}", ip, op, bytes);
                            ip += size;
                        } else {
                            break;
                        }
                    }
                    eprintln!("=== END {} ===", label);

                    match VM::new().run(chunk) {
                        Ok(v) => eprintln!("{} runtime: OK ({})", label, v),
                        Err(e) => eprintln!("{} runtime: FAIL - {}", label, e),
                    }
                }
                Err(e) => eprintln!("{} COMPILE ERR: {}", label, e),
            }
        }

        // Final assertion
        let mut vm = VM::new();
        let source =
            "i = 0; result = 0; while i < 500000 { result = result + i * 2 - 1 / 3; i = i + 1 }";
        let chunk = Compiler::compile(source).expect("B2 compilation should succeed");
        let result = vm.run(chunk);
        assert!(result.is_ok(), "B2 arithmetic loop should complete, got: {:?}", result.err());
    }

    // =========================================================================
    // NUD (Next-Use Distance) 集成测试
    // =========================================================================

    /// 验证 NUD 缩减寄存器后 GC 仍能正确追踪所有根
    ///
    /// 大量字符串变量 + 循环 → NUD 会缩减寄存器，
    /// GC 必须能正确追踪所有活跃的堆对象。
    #[test]
    fn test_nud_gc_safety() {
        use nuzo_compiler::Compiler;

        let source = r#"
a = "hello"
b = "world"
i = 0
while i < 100 {
    c = a + b
    i = i + 1
}
println(i)
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "NUD GC 安全测试运行应成功: {:?}", result.err());
    }

    /// 验证深递归 + NUD 寄存器复用下帧分页正确工作
    ///
    /// 递归函数 + 多个局部变量 → NUD 压缩寄存器，
    /// 帧分页必须在正确的寄存器窗口下工作。
    #[test]
    fn test_nud_deep_recursion_frame_paging() {
        use nuzo_compiler::Compiler;

        let source = r#"
fn fib(n) {
    if n <= 1 {
        return n
    }
    return fib(n - 1) + fib(n - 2)
}
println(fib(15))
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "NUD 深递归帧分页测试运行应成功: {:?}", result.err());
        // fib(15) = 610
        if let Ok(val) = result
            && val.is_smi()
        {
            assert_eq!(val.as_smi(), 610, "fib(15) 应为 610");
        }
    }

    // =========================================================================
    // 回归测试：BUG-CALL-CURRENT-BASE
    //
    // 根因：`setup_closure_frame` 和 `execute_closure_fast` 在拷贝参数时
    //   使用 `src_start = (func_reg + 1) as usize`，未加上 `current_base`。
    //   当 `current_base > 0`（即任何嵌套调用场景）时，会从错误的寄存器位置
    //   读取参数，导致被调用方收到 nil/旧值，最终触发 TypeMismatch。
    //
    // 修复：`src_start = caller_func_reg_abs + 1`（= current_base + func_reg + 1），
    //   对齐 `tail_call.rs` 的正确写法。
    //
    // 以下三个测试覆盖：正常路径（最小复现）、CSTS 快速路径、深层嵌套。
    // =========================================================================

    /// 正常路径：最小复现 — 函数 f 调用函数 g（current_base > 0 的最简场景）
    #[test]
    fn test_call_current_base_nested_closure_call() {
        use nuzo_compiler::Compiler;

        let source = r#"
fn g(n) { return n + 1 }
fn f(n) { return g(n - 1) }
println(f(5))
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "嵌套闭包调用应成功: {:?}", result.err());
        // f(5) = g(4) = 5
        if let Ok(val) = result
            && val.is_smi()
        {
            assert_eq!(val.as_smi(), 5, "f(5) 应为 5");
        }
    }

    /// CSTS 快速路径：多次调用同一调用点触发 Monomorphic 缓存，
    /// 验证 `execute_closure_fast` 修复后在热路径下仍正确传递参数。
    #[test]
    fn test_call_current_base_csts_fast_path() {
        use nuzo_compiler::Compiler;

        let source = r#"
fn double(n) { return n * 2 }
fn sum_with(a, b) { return a + b }
total = 0
i = 0
while i < 10 {
    total = sum_with(total, double(i))
    i = i + 1
}
println(total)
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "CSTS 快速路径嵌套调用应成功: {:?}", result.err());
        // double(0..9) = 0,2,4,...,18; sum = 90; sum_with 累加 → 90
        if let Ok(val) = result
            && val.is_smi()
        {
            assert_eq!(val.as_smi(), 90, "累加结果应为 90");
        }
    }

    /// 深层嵌套：3 层调用链（caller → f → g → h），验证 current_base 累加正确
    #[test]
    fn test_call_current_base_three_level_chain() {
        use nuzo_compiler::Compiler;

        let source = r#"
fn h(n) { return n - 1 }
fn g(n) { return h(n) * 2 }
fn f(n) { return g(n) + 10 }
println(f(7))
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "3 层嵌套调用应成功: {:?}", result.err());
        // f(7) = g(7) + 10 = (h(7) * 2) + 10 = (6 * 2) + 10 = 22
        if let Ok(val) = result
            && val.is_smi()
        {
            assert_eq!(val.as_smi(), 22, "f(7) 应为 22");
        }
    }

    /// 验证循环 + 大量局部变量下 NUD 正确分配
    ///
    /// 多个局部变量在循环中反复更新 → NUD 必须正确
    /// 分配和复用寄存器，不丢失变量值。
    #[test]
    fn test_nud_loop_with_many_locals() {
        use nuzo_compiler::Compiler;

        let source = r#"
a = 1
b = 2
c = 3
d = 4
e = 5
i = 0
while i < 50 {
    a = a + 1
    b = b + 1
    c = c + 1
    d = d + 1
    e = e + 1
    i = i + 1
}
println(a + b + c + d + e)
"#;
        let chunk = Compiler::compile(source).expect("编译应成功");
        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_ok(), "NUD 多局部变量循环测试运行应成功: {:?}", result.err());
    }

    // =========================================================================
    // ExecutionContext API 测试
    // =========================================================================

    #[test]
    fn test_execution_context_reset_registers_and_frames() {
        let mut vm = VM::new();
        // 先 push 一些值和帧
        vm.push(Value::from_number(1.0)).unwrap();
        vm.push(Value::from_number(2.0)).unwrap();
        vm.push_frame(None, 2).unwrap();
        assert!(!vm.cx.frame_metas.is_empty());

        // reset_registers_and_frames 应清空寄存器和帧
        vm.cx.reset_registers_and_frames(3);
        assert_eq!(vm.stack_size(), 3, "registers should be resized to locals_count");
        assert!(vm.cx.frame_metas.is_empty(), "frames should be cleared");
    }

    #[test]
    fn test_execution_context_snapshot_for_chunk_switch() {
        let mut vm = VM::new();
        vm.cx.snapshot_for_chunk_switch();
        // snapshot 后 running 应为 true
        assert!(vm.is_running(), "snapshot_for_chunk_switch should set running to true");
        // hot_trace_events 应为空
        assert!(vm.hot_trace_events().is_empty());
    }

    // =========================================================================
    // VM 构造器测试
    // =========================================================================

    #[test]
    fn test_vm_new_with_output_capture() {
        let (vm, buf) = VM::new_with_output_capture();
        assert!(!vm.is_running());
        assert_eq!(vm.stack_size(), 0);
        // buf 应该是一个有效的 Mutex<Vec<String>>
        let captured = buf.lock().unwrap();
        assert!(captured.is_empty());
    }

    #[test]
    fn test_vm_new_with_output_capture_and_tracer() {
        use crate::tracer_state::TraceConfig;
        let config = TraceConfig::default();
        let (vm, buf) = VM::new_with_output_capture_and_tracer(config);
        assert!(!vm.is_running());
        // tracer 启用后 instruction_count 应可调用（初始为 0）
        assert_eq!(vm.instruction_count(), 0);
        let captured = buf.lock().unwrap();
        assert!(captured.is_empty());
    }

    // =========================================================================
    // VM 状态查询 API 测试
    // =========================================================================

    #[test]
    fn test_vm_is_running_default_false() {
        let vm = VM::new();
        assert!(!vm.is_running());
    }

    #[test]
    fn test_vm_current_ip_default_zero() {
        let vm = VM::new();
        assert_eq!(vm.current_ip(), 0);
    }

    #[test]
    fn test_vm_instruction_count_no_tracer() {
        let vm = VM::new();
        // 没有 tracer 时返回 0
        assert_eq!(vm.instruction_count(), 0);
    }

    #[test]
    fn test_vm_stack_size_default_zero() {
        let vm = VM::new();
        assert_eq!(vm.stack_size(), 0);
    }

    #[test]
    fn test_vm_call_depth_default_zero() {
        let vm = VM::new();
        assert_eq!(vm.call_depth(), 0);
    }

    #[test]
    fn test_vm_pending_exception_default_none() {
        let vm = VM::new();
        assert!(vm.pending_exception().is_none());
    }

    #[test]
    fn test_vm_hot_trace_events_default_empty() {
        let vm = VM::new();
        assert!(vm.hot_trace_events().is_empty());
    }

    #[test]
    fn test_vm_last_call_stack_default_empty() {
        let vm = VM::new();
        assert!(vm.last_call_stack().is_empty());
    }

    // =========================================================================
    // VM 全局变量 API 测试
    // =========================================================================

    #[test]
    fn test_vm_add_global_returns_index() {
        let mut vm = VM::new();
        let builtin_count = vm.global_count();
        let idx = vm.add_global(Value::from_number(100.0));
        assert_eq!(idx, builtin_count);
        assert_eq!(vm.global_count(), builtin_count + 1);
    }

    #[test]
    fn test_vm_get_global_returns_value() {
        let mut vm = VM::new();
        let idx = vm.add_global(Value::from_number(55.0));
        let val = vm.get_global(idx).unwrap();
        assert_eq!(val.as_number(), 55.0);
    }

    #[test]
    fn test_vm_get_global_out_of_bounds_returns_none() {
        let vm = VM::new();
        assert!(vm.get_global(99999).is_none());
    }

    #[test]
    fn test_vm_define_global_returns_index() {
        let mut vm = VM::new();
        let builtin_count = vm.global_count();
        let idx = vm.define_global("my_var", Value::from_number(42.0));
        assert_eq!(idx, builtin_count);
    }

    #[test]
    fn test_vm_get_global_by_name_finds_value() {
        let mut vm = VM::new();
        vm.define_global("test_global", Value::from_number(77.0));
        let val = vm.get_global_by_name("test_global").unwrap();
        assert_eq!(val.as_number(), 77.0);
    }

    #[test]
    fn test_vm_get_global_by_name_not_found() {
        let vm = VM::new();
        assert!(vm.get_global_by_name("nonexistent_var").is_none());
    }

    #[test]
    fn test_vm_set_global_by_name_updates_existing() {
        let mut vm = VM::new();
        vm.define_global("counter", Value::from_number(1.0));
        vm.set_global_by_name("counter", Value::from_number(99.0));
        let val = vm.get_global_by_name("counter").unwrap();
        assert_eq!(val.as_number(), 99.0);
    }

    #[test]
    fn test_vm_set_global_by_name_creates_new() {
        let mut vm = VM::new();
        let before = vm.global_count();
        vm.set_global_by_name("new_var", Value::from_number(33.0));
        let after = vm.global_count();
        assert_eq!(after, before + 1);
        let val = vm.get_global_by_name("new_var").unwrap();
        assert_eq!(val.as_number(), 33.0);
    }

    #[test]
    fn test_vm_lookup_global_finds_value() {
        let mut vm = VM::new();
        vm.define_global("lookup_test", TRUE);
        let val = vm.lookup_global("lookup_test").unwrap();
        assert_eq!(val, TRUE);
    }

    #[test]
    fn test_vm_lookup_global_not_found() {
        let vm = VM::new();
        assert!(vm.lookup_global("missing").is_none());
    }

    #[test]
    fn test_vm_resolve_global_returns_index() {
        let mut vm = VM::new();
        let idx = vm.define_global("resolve_me", NIL);
        let resolved = vm.resolve_global("resolve_me").unwrap();
        assert_eq!(resolved, idx);
    }

    #[test]
    fn test_vm_resolve_global_not_found() {
        let vm = VM::new();
        assert!(vm.resolve_global("nope").is_none());
    }

    #[test]
    fn test_vm_global_count_increments() {
        let mut vm = VM::new();
        let initial = vm.global_count();
        vm.add_global(Value::from_number(1.0));
        vm.add_global(Value::from_number(2.0));
        assert_eq!(vm.global_count(), initial + 2);
    }

    #[test]
    fn test_vm_global_names_includes_defined() {
        let mut vm = VM::new();
        vm.define_global("alpha", Value::from_number(1.0));
        vm.define_global("beta", Value::from_number(2.0));
        let names = vm.global_names();
        assert!(names.contains(&"alpha".to_string()));
        assert!(names.contains(&"beta".to_string()));
    }

    // =========================================================================
    // VM 帧管理 API 测试
    // =========================================================================

    #[test]
    fn test_vm_push_frame_increments_depth() {
        let mut vm = VM::new();
        vm.push(Value::from_number(1.0)).unwrap();
        vm.push_frame(None, 1).unwrap();
        assert_eq!(vm.call_depth(), 1);
    }

    #[test]
    fn test_vm_push_frame_with_base_sets_base() {
        let mut vm = VM::new();
        vm.push_frame_with_base(0, 0, None, 0, None).unwrap();
        assert_eq!(vm.call_depth(), 1);
        // pop 后深度归零
        vm.pop_frame().unwrap();
        assert_eq!(vm.call_depth(), 0);
    }

    #[test]
    fn test_vm_frame_pager_stats_returns_valid_stats() {
        let vm = VM::new();
        let stats = vm.frame_pager_stats();
        // 初始状态下 spillover 和 restore 应为 0
        assert_eq!(stats.spill_count, 0);
        assert_eq!(stats.restore_count, 0);
    }

    // =========================================================================
    // VM 诊断模式 API 测试
    // =========================================================================

    #[test]
    fn test_vm_enable_diagnostic_mode() {
        let mut vm = VM::new();
        assert!(!vm.is_diagnostic_mode());
        vm.enable_diagnostic_mode();
        assert!(vm.is_diagnostic_mode());
    }

    #[test]
    fn test_vm_disable_diagnostic_mode() {
        let mut vm = VM::new();
        vm.enable_diagnostic_mode();
        assert!(vm.is_diagnostic_mode());
        vm.disable_diagnostic_mode();
        assert!(!vm.is_diagnostic_mode());
    }

    #[test]
    fn test_vm_is_diagnostic_mode_default_false() {
        let vm = VM::new();
        assert!(!vm.is_diagnostic_mode());
    }

    #[test]
    fn test_vm_with_max_diagnostic_errors_sets_limit() {
        let mut vm = VM::new();
        vm.with_max_diagnostic_errors(50);
        // 不 panic 即通过；可通过 error_collector_mut 间接验证
        let collector = vm.error_collector_mut();
        assert!(collector.error_count() == 0);
    }

    #[test]
    fn test_vm_with_stop_on_fatal_sets_flag() {
        let mut vm = VM::new();
        vm.with_stop_on_fatal(false);
        vm.with_stop_on_fatal(true);
        // 不 panic 即通过
    }

    #[test]
    fn test_vm_clear_diagnostics_no_op_when_empty() {
        let mut vm = VM::new();
        vm.clear_diagnostics();
        assert_eq!(vm.diagnostic_error_count(), 0);
        assert!(!vm.has_diagnostic_errors());
    }

    #[test]
    fn test_vm_diagnostic_error_count_default_zero() {
        let vm = VM::new();
        assert_eq!(vm.diagnostic_error_count(), 0);
    }

    #[test]
    fn test_vm_has_diagnostic_errors_default_false() {
        let vm = VM::new();
        assert!(!vm.has_diagnostic_errors());
    }

    #[test]
    fn test_vm_error_collector_mut_returns_mutable_ref() {
        let mut vm = VM::new();
        let collector = vm.error_collector_mut();
        assert_eq!(collector.error_count(), 0);
    }

    #[test]
    fn test_vm_print_diagnostic_report_does_not_panic() {
        let vm = VM::new();
        vm.print_diagnostic_report();
        // 不 panic 即通过
    }

    #[test]
    fn test_vm_diagnose_internal_error_returns_diagnosis() {
        let vm = VM::new();
        let error = InternalError::NoChunkLoaded;
        let diagnosis = vm.diagnose_internal_error(&error);
        // 没有 chunk 加载时，disassembly 应为 "<no chunk loaded>"
        assert_eq!(diagnosis.disassembly, "<no chunk loaded>");
        assert_eq!(diagnosis.error_ip, None);
        assert_eq!(diagnosis.call_stack_depth, 0);
    }

    // =========================================================================
    // VM GC 与其他 API 测试
    // =========================================================================

    #[test]
    fn test_vm_gc_mut_returns_mutable_ref() {
        let mut vm = VM::new();
        let gc = vm.gc_mut();
        // 验证返回的是可变引用（可以调用 &mut 方法）
        gc.set_gc_threshold(4096);
        assert_eq!(vm.gc().threshold(), 4096);
    }

    #[test]
    fn test_vm_local_info_default_empty() {
        let vm = VM::new();
        // 没有帧时，local_info 应返回空 Vec
        let info = vm.local_info();
        assert!(info.is_empty());
    }

    #[test]
    fn test_vm_local_info_returns_non_nil_registers() {
        let mut vm = VM::new();
        // reset_registers_and_frames 设置 register_write_ptr = locals_count
        // 这样 local_info() 的 [current_base .. register_write_ptr] 范围才非空
        vm.cx.reset_registers_and_frames(3);
        vm.set_register(0, Value::from_number(123.0)).unwrap();
        let info = vm.local_info();
        assert!(!info.is_empty(), "local_info should include non-NIL registers");
        let (_, val) = &info[0];
        assert_eq!(val.as_number(), 123.0);
    }

    #[test]
    fn test_vm_take_tracer_result_no_tracer_returns_none() {
        let mut vm = VM::new();
        assert!(vm.take_tracer_result().is_none());
    }

    #[test]
    fn test_vm_take_tracer_result_with_tracer_returns_some() {
        use crate::tracer_state::TraceConfig;
        let config = TraceConfig::default();
        let (mut vm, _buf) = VM::new_with_output_capture_and_tracer(config);
        let result = vm.take_tracer_result();
        assert!(result.is_some(), "take_tracer_result should return Some when tracer is active");
        // 第二次调用应返回 None（tracer 已被 take）
        assert!(vm.take_tracer_result().is_none());
    }

    #[test]
    fn test_vm_build_call_stack_for_debug_no_chunk_returns_none() {
        let vm = VM::new();
        // 没有 chunk 加载时返回 None
        assert!(vm.build_call_stack_for_debug().is_none());
    }

    // =========================================================================
    // VmObserver 测试
    // =========================================================================

    #[test]
    fn test_noop_vm_observer_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<crate::vm::NoopVmObserver>();
    }

    #[test]
    fn test_noop_vm_observer_default_callbacks() {
        use crate::vm::VmObserver;
        let obs = crate::vm::NoopVmObserver;
        // 默认回调应该是空操作，不 panic
        obs.on_will_execute(0, 0);
        use nuzo_signal::VmErrorInfo;
        obs.on_error(&VmErrorInfo {
            error_message: String::new(),
            opcode: None,
            ip: 0,
            call_depth: 0,
        });
    }

    #[test]
    fn test_vm_default_has_no_observer() {
        let vm = VM::new();
        // 默认构造的 VM 不应有观察者
        // 通过 NoopVmObserver 可构造来验证 trait 可用
        let _noop = crate::vm::NoopVmObserver;
        drop(vm);
    }

    // =========================================================================
    // VmObserver 边界测试
    // =========================================================================

    /// 验证 VM::new() 默认无 observer，observer() 返回 None
    #[test]
    fn test_vm_observer_none_is_noop() {
        let vm = VM::new();
        assert!(vm.observer().is_none(), "默认 VM 不应有观察者");
    }

    /// 验证通过 with_observer 设置自定义观察者后，observer() 返回 Some
    /// 且可以手动调用观察者方法
    #[test]
    fn test_vm_observer_custom_receives_calls() {
        use crate::vm::VmObserver;
        use std::sync::{Arc, Mutex};

        struct TestObserver {
            calls: Arc<Mutex<Vec<String>>>,
        }

        impl TestObserver {
            fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
                let calls = Arc::new(Mutex::new(Vec::new()));
                let obs = TestObserver { calls: Arc::clone(&calls) };
                (obs, calls)
            }
        }

        impl VmObserver for TestObserver {
            fn on_will_execute(&self, opcode: u8, ip: usize) {
                self.calls.lock().unwrap().push(format!("exec:{}:{}", opcode, ip));
            }
            fn on_error(&self, info: &nuzo_signal::VmErrorInfo) {
                self.calls.lock().unwrap().push(format!("error:{}", info.error_message));
            }
        }

        let (obs, calls) = TestObserver::new();
        let vm = VM::new().with_observer(Box::new(obs));
        assert!(vm.observer().is_some(), "设置观察者后 observer() 应返回 Some");

        // 手动调用 observer 方法验证回调生效
        vm.observer().unwrap().on_will_execute(1, 0);
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(calls.lock().unwrap()[0], "exec:1:0");
    }

    /// 验证 NoopVmObserver 的所有方法都是空操作（不 panic）
    #[test]
    fn test_observer_noop_does_nothing() {
        use crate::vm::VmObserver;
        let obs = crate::vm::NoopVmObserver;
        // on_will_execute 默认空实现，不应 panic
        obs.on_will_execute(0, 0);
        obs.on_will_execute(255, usize::MAX);
        // on_error 默认空实现，不应 panic
        obs.on_error(&nuzo_signal::VmErrorInfo {
            error_message: "test error".into(),
            opcode: Some(42),
            ip: 100,
            call_depth: 5,
        });
    }

    /// 验证 set_observer 可以在已构造的 VM 上原地设置观察者
    #[test]
    fn test_vm_set_observer_replaces_observer() {
        use crate::vm::VmObserver;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingObserver {
            count: Arc<AtomicUsize>,
        }

        impl VmObserver for CountingObserver {
            fn on_will_execute(&self, _opcode: u8, _ip: usize) {
                self.count.fetch_add(1, Ordering::SeqCst);
            }
        }

        let count1 = Arc::new(AtomicUsize::new(0));
        let count2 = Arc::new(AtomicUsize::new(0));

        let mut vm = VM::new();
        assert!(vm.observer().is_none());

        // 设置第一个观察者
        let obs1 = CountingObserver { count: Arc::clone(&count1) };
        vm.set_observer(Box::new(obs1));
        assert!(vm.observer().is_some());
        vm.observer().unwrap().on_will_execute(0, 0);
        assert_eq!(count1.load(Ordering::SeqCst), 1);

        // 替换为第二个观察者
        let obs2 = CountingObserver { count: Arc::clone(&count2) };
        vm.set_observer(Box::new(obs2));
        vm.observer().unwrap().on_will_execute(0, 0);
        // 第二个观察者应收到回调
        assert_eq!(count2.load(Ordering::SeqCst), 1);
    }

    /// 验证 VmObserver trait 是 Send + Sync（跨线程安全）
    #[test]
    fn test_vm_observer_is_send_sync() {
        fn assert_send_sync<T: ?Sized + Send + Sync>() {}
        assert_send_sync::<dyn crate::vm::VmObserver>();
        assert_send_sync::<Box<dyn crate::vm::VmObserver>>();
    }

    // =========================================================================
    // H5 Hot Trace 融合指令除零 Bug 回归测试
    // =========================================================================
    //
    // Bug 背景：
    //   `dispatch_table.rs` 的 `_op_mov_binaryop`（Hot Trace 融合执行
    //   `Mov + (Sub|Mul|Div|Pow)` 指令对）原代码未检查除数为零，导致在
    //   Hot Trace 优化路径下执行 `Div`/`Pow` 时除数为 0 会 panic。
    //
    // 修复方案：
    //   在 `_op_mov_binaryop` 添加 `needs_zero_check: bool` 参数：
    //   - Div/Pow 传入 `true`：在 f64 和 Smi 快速路径上检查除数是否为零
    //   - Sub/Mul 传入 `false`：零开销，不需要除零检查
    //   调用点见 `vm.rs` 第 544-547 行 `execute_hot_trace_batch`。
    //
    // 测试目标：
    //   1. 验证普通路径除零返回 DivByZero 错误（不 panic）
    //   2. 验证取模（%）除零返回 DivByZero 错误
    //   3. 验证变量除零（运行时才知道除数为 0）返回 DivByZero 错误
    //   4. 验证循环中除零不会因 Hot Trace 优化而 panic
    //   5. 验证 Hot Trace 融合路径下除零返回 DivByZero 错误

    /// H5 回归测试：基础循环除零测试的迭代次数。
    ///
    /// 远小于 Hot Trace 触发阈值（warming_threshold=500），用于验证
    /// 普通路径下的循环除零安全性，不会触发 Hot Trace 融合执行。
    const H5_BASIC_LOOP_ITERATIONS: i64 = 100;

    /// H5 回归测试：Hot Trace 触发的循环次数。
    ///
    /// 远大于 warming_threshold=500，确保循环体 IP 进入 Hot 状态，
    /// 触发 `execute_hot_trace_batch` 融合执行路径（含 `Mov + Div` 融合）。
    const H5_HOT_TRACE_TRIGGER_ITERATIONS: i64 = 2000;

    /// 测试整数除零返回 DivByZero 错误（普通路径）。
    ///
    /// 验证 `_op_div` 的 ZeroUnbox 快速路径正确检测除数为 0，
    /// 返回 `NuzoErrorKind::DivisionByZero` 而非 panic。
    #[test]
    fn test_div_by_zero_returns_error() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        let source = "z = 10 / 0";
        let chunk = Compiler::compile(source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_err(), "10 / 0 应返回除零错误，而非成功执行");

        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, NuzoErrorKind::DivisionByZero),
            "期望 DivisionByZero 错误，实际得到: {:?}",
            err.kind
        );
    }

    /// 测试取模（%）除零返回 DivByZero 错误。
    ///
    /// 验证 `_op_rem` 的 ZeroUnbox 快速路径正确检测除数为 0。
    #[test]
    fn test_mod_by_zero_returns_error() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        let source = "z = 10 % 0";
        let chunk = Compiler::compile(source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_err(), "10 % 0 应返回除零错误，而非成功执行");

        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, NuzoErrorKind::DivisionByZero),
            "期望 DivisionByZero 错误，实际得到: {:?}",
            err.kind
        );
    }

    /// 测试通过变量除零返回 DivByZero 错误。
    ///
    /// 此场景下除数在运行时才知道是 0，覆盖变量加载 + 除法的完整路径。
    /// 变量路径可能触发不同的 ZeroUnbox 分支（Smi 或 f64），确保
    /// 所有快速路径分支都有除零保护。
    #[test]
    fn test_div_by_zero_with_zero_var() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        let source = "y = 0; z = 10 / y";
        let chunk = Compiler::compile(source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_err(), "10 / y (y=0) 应返回除零错误，而非成功执行");

        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, NuzoErrorKind::DivisionByZero),
            "期望 DivisionByZero 错误，实际得到: {:?}",
            err.kind
        );
    }

    /// 测试循环中除零不会因 Hot Trace 优化而 panic。
    ///
    /// 此测试验证：
    /// 1. 循环中除零应返回 DivByZero 错误，而非 panic
    /// 2. 第一次迭代即除零，不会触发 Hot Trace（hit_count 不足）
    ///
    /// 注意：此测试循环次数远小于 Hot Trace 触发阈值（500），
    /// 主要验证普通路径下循环除零的安全性。
    #[test]
    fn test_div_by_zero_in_loop_returns_error() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        // 构造循环源码：第一次迭代即除零
        let source =
            format!("i = 0; while i < {} {{ z = 10 / 0; i = i + 1 }}", H5_BASIC_LOOP_ITERATIONS);
        let chunk = Compiler::compile(&source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_err(), "循环中除零应返回除零错误，而非 panic 或成功");

        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, NuzoErrorKind::DivisionByZero),
            "期望 DivisionByZero 错误，实际得到: {:?}",
            err.kind
        );
    }

    /// 测试 Hot Trace 融合路径下的除零安全性（H5 bug 核心回归测试）。
    ///
    /// 此测试设计用于触发 Hot Trace 优化（循环 2000 次 >> warming_threshold=500），
    /// 在循环末尾自然产生除数为 0 的情况，验证 Hot Trace 融合路径下的
    /// `Mov + Div` 除零检查（`needs_zero_check = true`）能正确返回错误，而非 panic。
    ///
    /// # 设计原理
    ///
    /// 1. 循环变量 i 从 `H5_HOT_TRACE_TRIGGER_ITERATIONS` 递减到 0
    /// 2. 前 2000 次迭代除数非零（i = 2000..1），累积 hit_count 触发 Hot Trace
    /// 3. 最后一次迭代 i = 0，`10 / i` 触发除零
    /// 4. 若 Hot Trace 已触发，会走 `execute_hot_trace_batch`，其中
    ///    `Mov + Div` 相邻指令对会调用 `_op_mov_binaryop(..., true)`
    /// 5. 融合路径的除零检查应返回 DivByZero 错误
    ///
    /// # 优势
    ///
    /// 此方案无需 `if` 语句动态改变除数，循环变量自然递减到 0，
    /// 同时保证了 Hot Trace 的触发条件（足够多的成功迭代）。
    ///
    /// # 注意
    ///
    /// Hot Trace 是否真正触发取决于编译器生成的字节码模式（是否包含
    /// `Mov + Div` 相邻指令）。即使 Hot Trace 未触发，普通 `_op_div`
    /// 路径也有除零检查，此测试仍能验证循环中除零的安全性。
    #[test]
    fn test_hot_trace_div_by_zero_safety() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        // 循环变量 i 从 H5_HOT_TRACE_TRIGGER_ITERATIONS 递减到 0
        // 前 2000 次除数非零（成功执行，触发 Hot Trace），最后一次 i=0 除零
        let source = format!(
            "i = {}; while i >= 0 {{ z = 10 / i; i = i - 1 }}",
            H5_HOT_TRACE_TRIGGER_ITERATIONS
        );
        let chunk = Compiler::compile(&source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_err(), "Hot Trace 路径下除零应返回除零错误，而非 panic 或成功");

        let err = result.unwrap_err();
        assert!(
            matches!(err.kind, NuzoErrorKind::DivisionByZero),
            "期望 DivisionByZero 错误，实际得到: {:?}",
            err.kind
        );
    }

    // =========================================================================
    // H7/H8 u16 截断 Bug 回归测试
    // =========================================================================
    //
    // Bug 背景：
    //   VM 在多处对寄存器编号 / 全局变量索引 / 版本号执行 `as u16` 转换，
    //   当源值超过 u16::MAX (65535) 时会发生静默截断，导致：
    //   - 寄存器索引回绕 → 读写错误寄存器（数据损坏）
    //   - 全局变量索引回绕 → 访问幽灵全局变量
    //   - 版本号截断 → GetGlobalCached 永远版本不匹配（死循环冷路径）
    //
    // 修复方案：
    //   在 5 个截断点添加显式边界检查，超限时返回 InternalError 而非截断：
    //   1. dispatch.rs `call_builtin_unrolled` (argc > 3)：func_reg + 1 + argc 检查
    //   2. dispatch.rs `op_array_new`：dest + 1 + count 检查 → RegisterOverflow
    //   3. dispatch.rs `op_get_global` ISS patch：idx/ver 检查 → GlobalIndexOverflow
    //   4. dispatch.rs `op_get_global_cached` 冷路径：new_ver 检查 → GlobalIndexOverflow
    //   5. variable_ops.rs `push`：registers.len() 检查 → RegisterOverflow
    //
    // 测试目标：
    //   验证边界检查生效，超限时返回正确错误变体，而非静默截断。

    /// H7/H8 回归测试：push 在寄存器数量超过 u16::MAX 时返回 RegisterOverflow。
    ///
    /// 验证修复点5（variable_ops.rs `push`）。
    ///
    /// 原理：连续调用 `VM::push` 直到寄存器文件长度达到 u16::MAX + 1，
    /// 此时 push 应拒绝写入并返回 `InternalError::RegisterOverflow { count }`，
    /// 而非将索引截断为 u16 导致后续 pop/peek 错位。
    ///
    /// 内存消耗：u16::MAX + 1 = 65536 个 Value（每个 8 字节），约 512 KB，可接受。
    #[test]
    fn test_push_returns_register_overflow_at_u16_boundary() {
        let mut vm = VM::new();
        // 填充寄存器至 idx = u16::MAX（共 u16::MAX + 1 = 65536 次 push，全部成功）
        for _ in 0..=u16::MAX {
            vm.push(NIL).expect("索引在 u16 范围内时 push 应成功");
        }
        assert_eq!(vm.stack_size(), u16::MAX as usize + 1);

        // 下一次 push：idx = u16::MAX + 1，超过 u16 域 → 应返回 RegisterOverflow
        let result = vm.push(NIL);
        assert!(result.is_err(), "超过 u16::MAX 时 push 应返回错误而非截断索引");
        let err = result.unwrap_err();
        match err.kind {
            NuzoErrorKind::Internal(InternalError::RegisterOverflow { count }, _) => {
                assert_eq!(
                    count,
                    u16::MAX as usize + 1,
                    "count 应为 u16::MAX + 1（即首次溢出的索引）"
                );
            }
            other => panic!("期望 RegisterOverflow 错误，实际得到: {:?}", other),
        }
    }

    /// H7/H8 回归测试：op_array_new 在 dest + count 超过 u16::MAX 时返回 RegisterOverflow。
    ///
    /// 验证修复点2（dispatch.rs `op_array_new`）。
    ///
    /// 原理：构造字节码 `ArrayNew dest=0 count=u16::MAX`，此时
    /// `last_reg = dest + 1 + count = 0 + 1 + u16::MAX = u16::MAX + 1 > u16::MAX`，
    /// 边界检查应拦截并返回 `RegisterOverflow { count: u16::MAX + 1 }`，
    /// 而非让后续 `as u16` 截断导致读取错误寄存器。
    #[test]
    fn test_op_array_new_register_overflow_on_count_overflow() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::ArrayNew);
        chunk.write_u16(0); // dest = 0
        chunk.write_u16(u16::MAX); // count = u16::MAX → last_reg = u16::MAX + 1 > u16::MAX
        chunk.write_opcode(Opcode::Halt);

        let mut vm = VM::new();
        let result = vm.run(chunk);
        assert!(result.is_err(), "ArrayNew count=u16::MAX 应返回 RegisterOverflow 而非截断");
        let err = result.unwrap_err();
        match err.kind {
            NuzoErrorKind::Internal(InternalError::RegisterOverflow { count }, _) => {
                assert_eq!(
                    count,
                    u16::MAX as usize + 1,
                    "count 应为 dest(0) + 1 + u16::MAX = u16::MAX + 1"
                );
            }
            other => panic!("期望 RegisterOverflow 错误，实际得到: {:?}", other),
        }
    }

    /// H7/H8 回归测试：op_get_global ISS patch 在版本号超过 u16::MAX 时返回 GlobalIndexOverflow。
    ///
    /// 验证修复点3（dispatch.rs `op_get_global` ISS patch 路径）。
    ///
    /// 原理：预注册全局变量 "x" 并手动设置其版本号为 u16::MAX + 1
    /// （模拟 65536 次 set_global 后的状态，因 set_global 每次递增版本号），
    /// 然后执行 `GetGlobal` 指令。首次 resolve 成功后进入 ISS patch 路径，
    /// 边界检查发现 `ver > u16::MAX`，应返回 `GlobalIndexOverflow { idx, ver }`，
    /// 而非将版本号截断写入指令导致 GetGlobalCached 版本永不匹配。
    ///
    /// 注意：版本号通过直接设置 `vm.cx.global_versions` 模拟，避免真正执行
    /// 65536 次 set_global 的性能开销。`reset_and_load_chunk` 不会清除
    /// `global_scope` 和 `global_versions`，只清除 `global_cache`，
    /// 因此预设状态在 `run` 后仍然有效。
    #[test]
    fn test_op_get_global_returns_global_index_overflow_on_version_overflow() {
        let mut vm = VM::new();
        // 注册全局变量 "x"（run 后仍然存在，因 reset 不清除 global_scope）
        vm.set_global_by_name("x", NIL);
        let idx = vm.resolve_global("x").expect("全局变量 x 应已注册");

        // 手动设置版本号超过 u16::MAX（模拟 65536 次 set_global 递增后的状态）
        if idx >= vm.cx.global_versions.len() {
            vm.cx.global_versions.resize(idx + 1, 0);
        }
        let overflow_ver = u16::MAX as u32 + 1; // 65536
        vm.cx.global_versions[idx] = overflow_ver;

        // 构造字节码：GetGlobal dest=0 name_idx=const("x") iss_pad=0; Halt
        let mut chunk = Chunk::new();
        chunk.locals_count = 1; // dest=0 需要至少 1 个局部寄存器供 set_register 使用
        let name_idx = chunk.add_constant(Value::from_string("x"));
        chunk.write_opcode(Opcode::GetGlobal);
        chunk.write_u16(0); // dest = 0
        chunk.write_u16(name_idx as u16); // name_idx
        chunk.write_u16(0); // iss_pad（将被 patch 为 version）
        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(
            result.is_err(),
            "GetGlobal 版本号超过 u16::MAX 应返回 GlobalIndexOverflow 而非截断"
        );
        let err = result.unwrap_err();
        match err.kind {
            NuzoErrorKind::Internal(
                InternalError::GlobalIndexOverflow { idx: err_idx, ver },
                _,
            ) => {
                assert_eq!(err_idx, idx, "idx 应为全局变量 x 的索引");
                assert_eq!(ver, overflow_ver, "ver 应为 u16::MAX + 1");
            }
            other => panic!("期望 GlobalIndexOverflow 错误，实际得到: {:?}", other),
        }
    }

    /// H7/H8 回归测试：op_get_global_cached 冷路径在版本号超过 u16::MAX 时返回 GlobalIndexOverflow。
    ///
    /// 验证修复点4（dispatch.rs `op_get_global_cached` 冷路径）。
    ///
    /// 原理：预注册全局变量 "x" 并手动设置其版本号为 u16::MAX + 1，
    /// 然后执行 `GetGlobalCached` 指令（带一个不匹配的 expected_ver=0）。
    /// 版本不匹配进入冷路径，重新读取值后尝试 patch 新版本号，
    /// 边界检查发现 `new_ver > u16::MAX`，应返回 `GlobalIndexOverflow { idx, ver }`，
    /// 而非截断版本号导致指令永远版本不匹配（死循环冷路径）。
    ///
    /// 与修复点3的区别：修复点3检查 ISS patch 时的版本号，修复点4检查
    /// 冷路径重新 patch 时的版本号。两者检查时机不同，需分别覆盖。
    #[test]
    fn test_op_get_global_cached_returns_global_index_overflow_on_cold_path() {
        let mut vm = VM::new();
        // 注册全局变量 "x" 并设置值（冷路径需要 get_global 成功返回值）
        vm.set_global_by_name("x", NIL);
        let idx = vm.resolve_global("x").expect("全局变量 x 应已注册");

        // 手动设置版本号超过 u16::MAX
        if idx >= vm.cx.global_versions.len() {
            vm.cx.global_versions.resize(idx + 1, 0);
        }
        let overflow_ver = u16::MAX as u32 + 1; // 65536
        vm.cx.global_versions[idx] = overflow_ver;

        // 构造字节码：GetGlobalCached dest=0 gidx=idx ver=0（不匹配，触发冷路径）; Halt
        // idx 远小于 u16::MAX（仅注册了 1 个全局变量），可安全 as u16
        let mut chunk = Chunk::new();
        chunk.locals_count = 1; // dest=0 需要至少 1 个局部寄存器
        chunk.write_opcode(Opcode::GetGlobalCached);
        chunk.write_u16(0); // dest = 0
        chunk.write_u16(idx as u16); // gidx
        chunk.write_u16(0); // expected_ver = 0（与 overflow_ver 不匹配 → 冷路径）
        chunk.write_opcode(Opcode::Halt);

        let result = vm.run(chunk);
        assert!(
            result.is_err(),
            "GetGlobalCached 冷路径版本号超过 u16::MAX 应返回 GlobalIndexOverflow 而非截断"
        );
        let err = result.unwrap_err();
        match err.kind {
            NuzoErrorKind::Internal(
                InternalError::GlobalIndexOverflow { idx: err_idx, ver },
                _,
            ) => {
                assert_eq!(err_idx, idx, "idx 应为全局变量 x 的索引");
                assert_eq!(ver, overflow_ver, "ver 应为 u16::MAX + 1");
            }
            other => panic!("期望 GlobalIndexOverflow 错误，实际得到: {:?}", other),
        }
    }

    // 注：修复点1（dispatch.rs `call_builtin_unrolled` argc > 3 路径）未单独编写测试。
    // 原因：触发该路径需要 builtin 函数接受 > u16::MAX 个参数，或 func_reg 接近 u16::MAX
    // 且参数数量足以让 func_reg + 1 + argc > u16::MAX，这在实际场景中难以构造。
    // 该检查与修复点2（op_array_new）使用相同的 `last_reg > u16::MAX` 边界检查模式，
    // 已由 test_op_array_new_register_overflow_on_count_overflow 间接验证模式正确性。

    // =========================================================================
    // S1-S4 P0 回归测试 + P1 GC 压力/边界测试
    // =========================================================================
    //
    // 审核报告审核出 runtime 模块 4 个 P0 严重问题（S1-S4）：
    //   S1: Hot Trace lookahead off-by-one（vm.rs:895）
    //   S2: safe_point 非传递性提升（gc/alloc.rs:405）
    //   S3: mark 阶段跳过 scratch 对象（gc/mark.rs:152+247）
    //   S4: arena 提升后悬垂引用未重写（call_dispatch.rs:330）
    //
    // 以下测试逐个验证修复的正确性。

    /// S1 回归测试：Hot Trace 融合后 next_ip 计算正确。
    ///
    /// 原 bug：`execute_hot_trace_batch` 中 `next_ip = self.ip + instruction_size()`，
    /// 但 `fetch_opcode()` 已通过 `read_byte()` 把 `self.ip` 前进 1 字节，
    /// 故 `self.ip` 指向操作数字节而非 opcode。`instruction_size()` 返回
    /// 完整指令长度（1 opcode + N 操作数），导致 `next_ip` 多前进 1 字节，
    /// 跳过下一条指令的 opcode 字节，把其操作数字节误读为 opcode。
    ///
    /// 修复：`next_ip = self.ip + opcode.instruction_size() - 1`
    ///
    /// 测试原理：编译 3000 次迭代的循环计算 sum = 1+2+...+3000 = 4501500。
    /// 足够多的迭代触发 Hot Trace 融合路径。若 IP 偏移 1 字节，
    /// 循环体会被破坏 → 结果错误或 VM panic。
    #[test]
    fn test_s1_hot_trace_fusion_correct() {
        use nuzo_compiler::Compiler;

        let mut vm = VM::new();
        let source = "sum = 0; i = 1; while i <= 3000 { sum = sum + i; i = i + 1 }";
        let chunk = Compiler::compile(source).expect("编译应成功");

        let result = vm.run(chunk);
        assert!(result.is_ok(), "Hot Trace 融合后 IP 错误导致执行失败: {:?}", result.err());

        let sum_val = vm.get_global_by_name("sum").expect("全局变量 sum 应存在");
        let sum = sum_val.as_number();
        // sum = 3000 * 3001 / 2 = 4501500
        assert_eq!(
            sum, 4501500.0,
            "sum = 1+2+...+3000 应为 4501500，实际: {}（若 IP off-by-one 则结果错误）",
            sum
        );
    }

    /// S2 回归测试：safe_point 传递性提升 scratch 对象内部引用。
    ///
    /// 原 bug：`safe_point` 提升后只重写根引用（caller 寄存器/全局变量），
    /// 不重写被提升对象内部对其他 scratch 对象的引用 → 被提升对象 A 内部
    /// 仍持有 scratch 索引指向 B → scratch_top 重置后旧索引悬垂 → UAF。
    ///
    /// 修复：在 `safe_point` 的提升循环后，对所有已提升对象调用
    /// `remap_scratch_indices(&remap)`，把内部 scratch 引用重写为新堆索引。
    ///
    /// 测试原理：
    /// 1. 分配 scratch B（Array 含数字 42）
    /// 2. 分配 scratch A（Array 含 Value 指向 B 的 scratch 索引）
    /// 3. safe_point(A, B 均为根) → 两者提升到持久堆
    /// 4. 验证 A 内部引用已重写为 B 的新堆索引（传递性重写）
    #[test]
    fn test_s2_safe_point_transitive_promotion() {
        let mut gc = Gc::with_default_threshold();

        // 分配 scratch B（Array 含数字 42）
        let b = gc.alloc_scratch(HeapObject::Array(vec![Value::from_number(42.0)]));
        assert!(is_scratch(b), "B 应为 scratch 索引");

        // 分配 scratch A（Array 含 Value 指向 B 的 scratch 索引）
        let a = gc.alloc_scratch(HeapObject::Array(vec![Value::from_scratch_index(b)]));
        assert!(is_scratch(a), "A 应为 scratch 索引");

        // safe_point：A 和 B 均为根，应被提升
        let remap = gc.safe_point(|| vec![a, b]);
        assert_eq!(remap.len(), 2, "A 和 B 都应被提升");

        // 查找 A 和 B 的新堆索引
        let new_a =
            remap.iter().find(|&&(old, _)| old == a).map(|&(_, n)| n).expect("A 应在 remap 中");
        let new_b =
            remap.iter().find(|&&(old, _)| old == b).map(|&(_, n)| n).expect("B 应在 remap 中");
        assert!(!is_scratch(new_a), "A 的新索引应在持久堆");
        assert!(!is_scratch(new_b), "B 的新索引应在持久堆");

        // 验证 A 内部引用已重写为 B 的新堆索引（传递性重写）
        match gc.get(new_a).expect("gc.get should succeed for valid index") {
            HeapObject::Array(elements) => {
                assert_eq!(elements.len(), 1, "A 应有 1 个元素");
                let inner = elements[0];
                assert_eq!(
                    inner.heap_index(),
                    Some(new_b),
                    "A[0] 应指向 B 的新堆索引 {}，实际: {:?}（若未传递性重写则为旧 scratch 索引）",
                    new_b,
                    inner.heap_index()
                );
            }
            other => panic!("A 应为 Array，实际: {:?}", other),
        }
    }

    /// S3 回归测试：mark 阶段不跳过 scratch 对象。
    ///
    /// 原 bug：`process_wave_front_step` 中 scratch 索引的 `chunk_id(idx)` 远超
    /// `chunks.len()`，命中 `cid >= self.chunks.len()` 提前返回 → scratch 对象
    /// 不被 trace → 其引用的堆对象不被标记 → 被错误回收。
    ///
    /// 修复：在 `cid >= chunks.len()` 检查前添加 scratch 处理分支，
    /// 用 `scratch_mark_epoch` 防止循环。
    ///
    /// 测试原理：
    /// 1. 分配堆对象 H（Array 含数字 111）— 应通过 S 追溯到并存活
    /// 2. 分配 scratch S（Array 含 Value 指向 H）
    /// 3. 分配堆对象 G（垃圾，不可达，应被回收）
    /// 4. collect_with_roots(S 为唯一根)
    /// 5. 验证 H 存活（S3 修复后 S 被 trace → H 被标记）
    /// 6. 验证 G 被回收（不可达）
    #[test]
    fn test_s3_mark_scratch_not_skipped() {
        let mut gc = Gc::with_default_threshold();

        // 分配堆对象 H（应通过 scratch S 追溯到并存活）
        let h = gc.alloc(HeapObject::Array(vec![Value::from_number(111.0)]));
        // 分配 scratch S（Array 含 Value 指向 H）
        let s = gc.alloc_scratch(HeapObject::Array(vec![Value::from_gc_index(h)]));
        // 分配垃圾堆对象 G（不可达，应被回收）
        let g = gc.alloc(HeapObject::Array(vec![Value::from_number(999.0)]));

        // GC：以 S 为唯一根
        gc.collect_with_roots(std::iter::once(Value::from_scratch_index(s)));

        // 验证 H 存活（通过 S → H 追溯，S3 修复后 S 不被跳过）
        assert!(
            gc.try_get(h).is_some(),
            "H 应存活（通过 scratch S 追溯），但被错误回收（S3 bug：S 被 mark 跳过）"
        );
        // 验证 G 被回收（不可达）
        assert!(gc.try_get(g).is_none(), "G 应被回收（不可达），但仍然存在");
    }

    /// S4 回归测试：arena 提升后 caller 寄存器引用被重写。
    ///
    /// 原 bug：`promote_arena_range` 调用 `promote_from_region(obj, size_est)`
    /// 后丢弃返回的 `new_heap_idx` → caller 寄存器仍持有旧 arena 索引 →
    /// 帧结束后 arena 内存被复用 → 悬垂指针 → UAF。
    ///
    /// 修复：收集 `(arena_offset, new_heap_idx)` remap，提升后遍历
    /// caller 寄存器 / global_scope / frame_data，把 arena 索引重写为堆索引。
    ///
    /// 测试原理：
    /// 1. 在 VM 的 region 中创建 arena 帧并分配 arena 对象
    /// 2. 将 arena Value 放入 caller 寄存器[0]
    /// 3. end_frame(has_escape=true)
    /// 4. promote_arena_range(frame_idx, caller_base=1)
    /// 5. 验证寄存器[0] 不再是 arena 索引，而是有效堆索引
    /// 6. 验证堆对象内容正确
    #[test]
    fn test_s4_no_dangling_pointer() {
        let mut vm = VM::new();

        // 1. 创建 arena 帧
        let frame_idx = vm.cx.region.begin_frame();

        // 2. 在 arena 中分配对象（Array 含数字 77）
        let arena_val = vm
            .cx
            .region
            .allocate_object(frame_idx, HeapObject::Array(vec![Value::from_number(77.0)]), 32)
            .expect("arena 分配应成功（arena 默认启用）");
        assert!(arena_val.try_arena_offset().is_some(), "分配的 Value 应为 arena 索引");

        // 3. 将 arena_val 放入寄存器[0]（caller 寄存器）
        vm.push(arena_val).expect("push 应成功");
        assert_eq!(vm.stack_size(), 1);

        // 4. promote_arena_range 必须在 end_frame 之前调用（对齐生产路径修复）。
        //    原顺序（end_frame → promote）有 bug：end_frame 会 truncate frame_stack
        //    销毁 ArenaFrameState，导致 promote_arena_range 内 frame_objects()/
        //    frame_state() 返回 None → 提前返回 → 提升从未发生 → 寄存器仍持
        //    旧 arena 索引 → 悬垂指针 → UAF。
        //    正确顺序：promote（frame 仍在，可读 obj_start/obj_count，take 对象）
        //    → end_frame（truncate frame_stack，对象已被 take 走）。
        vm.promote_arena_range(frame_idx, 1).expect("arena 提升应成功");

        // 5. end_frame 清理帧（promote 已取走对象，end_frame 仅 truncate frame_stack）
        let escape_result = vm.cx.region.end_frame(frame_idx, true);
        assert!(escape_result.is_some(), "有逃逸时应返回 Some");

        // 6. 验证寄存器[0] 已被重写为堆索引（非 arena 索引）
        let reg_slice = vm.cx.registers.as_slice();
        assert!(!reg_slice.is_empty(), "寄存器应至少有 1 个元素");
        let reg_val = reg_slice[0];
        assert!(
            reg_val.try_arena_offset().is_none(),
            "寄存器[0] 不应再是 arena 索引（应已重写为堆索引）"
        );
        let new_heap_idx = reg_val.heap_index().expect("应为有效堆索引");
        assert!(!is_scratch(new_heap_idx), "不应是 scratch 索引");

        // 7. 验证堆对象内容正确（通过 VM 的 gc 访问器读取）
        match vm.gc().get(new_heap_idx).expect("gc.get should succeed for valid index") {
            HeapObject::Array(elements) => {
                assert_eq!(elements.len(), 1);
                assert_eq!(
                    elements[0].as_number(),
                    77.0,
                    "堆对象内容应为 77（原 arena 对象的内容）"
                );
            }
            other => panic!("堆对象应为 Array，实际: {:?}", other),
        }
    }

    // =========================================================================
    // P1 GC 压力/边界测试
    // =========================================================================

    /// P1 压力测试：大量 scratch 对象提升后全部存活。
    ///
    /// 分配 100 个 scratch 对象（每个含不同数字），全部作为根提升，
    /// 验证 remap 包含全部 100 项且每个新堆对象内容正确。
    /// 覆盖 safe_point 在高压力下的正确性。
    #[test]
    fn test_gc_stress_scratch_promotion_many_objects() {
        let mut gc = Gc::with_default_threshold();
        let count = 100;
        let mut scratch_indices = Vec::with_capacity(count);

        // 分配 100 个 scratch 对象
        for i in 0..count {
            let idx = gc.alloc_scratch(HeapObject::Array(vec![Value::from_number(i as f64)]));
            scratch_indices.push(idx);
        }

        // 全部作为根提升
        let indices_clone = scratch_indices.clone();
        let remap = gc.safe_point(|| indices_clone.clone());
        assert_eq!(remap.len(), count, "所有 {} 个 scratch 对象都应被提升", count);

        // 验证每个新堆对象内容正确
        for (i, &old_idx) in scratch_indices.iter().enumerate() {
            let new_idx = remap
                .iter()
                .find(|&&(old, _)| old == old_idx)
                .map(|&(_, n)| n)
                .unwrap_or_else(|| panic!("scratch 对象 {} 应在 remap 中", i));
            match gc.get(new_idx).expect("gc.get should succeed for valid index") {
                HeapObject::Array(elements) => {
                    assert_eq!(elements[0].as_number(), i as f64, "对象 {} 内容应为 {}", i, i);
                }
                other => panic!("对象 {} 应为 Array，实际: {:?}", i, other),
            }
        }
    }

    /// P1 边界测试：3 层嵌套 scratch 引用的传递性重写。
    ///
    /// A(scratch) → B(scratch) → C(scratch) → 数字 42
    /// safe_point 后，A、B、C 均提升到堆。
    /// 验证 A→B→C 的引用链全部被传递性重写（不限于 1 层）。
    #[test]
    fn test_gc_boundary_nested_scratch_references_3_levels() {
        let mut gc = Gc::with_default_threshold();

        // C: scratch Array 含数字 42
        let c = gc.alloc_scratch(HeapObject::Array(vec![Value::from_number(42.0)]));
        // B: scratch Array 含 Value 指向 C
        let b = gc.alloc_scratch(HeapObject::Array(vec![Value::from_scratch_index(c)]));
        // A: scratch Array 含 Value 指向 B
        let a = gc.alloc_scratch(HeapObject::Array(vec![Value::from_scratch_index(b)]));

        // safe_point：A、B、C 均为根
        let remap = gc.safe_point(|| vec![a, b, c]);
        assert_eq!(remap.len(), 3, "A、B、C 都应被提升");

        let new_a = remap.iter().find(|&&(old, _)| old == a).map(|&(_, n)| n).expect("A");
        let new_b = remap.iter().find(|&&(old, _)| old == b).map(|&(_, n)| n).expect("B");
        let new_c = remap.iter().find(|&&(old, _)| old == c).map(|&(_, n)| n).expect("C");

        // 验证 A → B → C 引用链全部重写
        match gc.get(new_a).expect("gc.get should succeed for valid index") {
            HeapObject::Array(elements) => {
                assert_eq!(elements[0].heap_index(), Some(new_b), "A[0] 应指向 B 的新索引");
            }
            other => panic!("A 应为 Array，实际: {:?}", other),
        }
        match gc.get(new_b).expect("gc.get should succeed for valid index") {
            HeapObject::Array(elements) => {
                assert_eq!(elements[0].heap_index(), Some(new_c), "B[0] 应指向 C 的新索引");
            }
            other => panic!("B 应为 Array，实际: {:?}", other),
        }
        match gc.get(new_c).expect("gc.get should succeed for valid index") {
            HeapObject::Array(elements) => {
                assert_eq!(elements[0].as_number(), 42.0, "C[0] 应为 42");
            }
            other => panic!("C 应为 Array，实际: {:?}", other),
        }
    }
}
