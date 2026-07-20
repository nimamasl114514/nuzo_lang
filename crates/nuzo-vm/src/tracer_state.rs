use nuzo_bytecode::Opcode;
use nuzo_core::SourceLocation;
use nuzo_core::Value;
use nuzo_core::encoding::utf8_truncate;
use nuzo_values::ValueExt;

#[derive(Debug, Clone)]
pub struct TraceConfig {
    pub filter_opcodes: Option<Vec<Opcode>>,
    pub capture_registers: bool,
    pub max_entries: Option<usize>,
    pub register_window: Option<usize>,
}

impl Default for TraceConfig {
    fn default() -> Self {
        TraceConfig {
            filter_opcodes: None,
            capture_registers: false,
            max_entries: None,
            register_window: Some(DEFAULT_REGISTER_WINDOW),
        }
    }
}

const DEFAULT_REGISTER_WINDOW: usize = 16;

/// 整数显示阈值：绝对值小于此值的整数部分无精度损失的浮点数，以整数格式显示
const INTEGER_DISPLAY_THRESHOLD: f64 = 1e15;

/// 字符串预览最大字符数（超出则截断并追加 "..."）
const STRING_PREVIEW_LEN: usize = 20;

/// 字符串截断后保留的字符数（= STRING_PREVIEW_LEN - "..." 占 3 字符）
const STRING_PREVIEW_KEEP: usize = STRING_PREVIEW_LEN - 3;

#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub instruction_index: usize,
    pub opcode: Opcode,
    pub operands: Vec<u16>,
    pub ip_before: usize,
    pub ip_after: usize,
    pub frame_depth: usize,
    pub registers_before: Option<Vec<String>>,
    pub registers_after: Option<Vec<String>>,
    pub duration_ns: u128,
    /// 源码位置（文件:行:列），从 Chunk 的 debug info 中提取
    pub source_location: Option<SourceLocation>,
    /// 当前帧所属函数名（顶层为 "<script>"）
    pub function_name: Option<String>,
}

impl std::fmt::Display for TraceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref fn_name) = self.function_name {
            write!(f, "[{}] ", fn_name)?;
        }

        write!(
            f,
            "[{:04}] {:12} | ip={:04}->{:04} | depth={}",
            self.instruction_index, self.opcode, self.ip_before, self.ip_after, self.frame_depth
        )?;

        if let Some(ref loc) = self.source_location {
            write!(f, " | {}", loc)?;
        }

        if let (Some(before), Some(after)) = (&self.registers_before, &self.registers_after) {
            let before_str: String = before.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let after_str: String = after.iter().take(4).cloned().collect::<Vec<_>>().join(", ");

            write!(f, " | before=[{{ {before_str} }}] after=[{{ {after_str} }}]")?;
        }

        write!(f, " | {}ns", self.duration_ns)?;

        Ok(())
    }
}

#[derive(Debug)]
pub struct TraceResult {
    pub entries: Vec<TraceEntry>,
    pub total_instructions: usize,
    pub total_duration_ns: u128,
    pub config: TraceConfig,
}

impl TraceResult {
    pub fn filtered_count(&self) -> usize {
        self.total_instructions.saturating_sub(self.entries.len())
    }

    pub fn format_trace(&self) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "=== Trace Result ===\n\
             Total instructions: {}\n\
             Traced entries: {} (filtered: {})\n\
             Total duration: {}ns ({:.2}ms)\n\
             Config: {:?}\n\n",
            self.total_instructions,
            self.entries.len(),
            self.filtered_count(),
            self.total_duration_ns,
            self.total_duration_ns as f64 / 1_000_000.0,
            self.config
        ));

        for entry in &self.entries {
            output.push_str(&format!("{}\n", entry));
        }

        output
    }

    pub fn max_frame_depth(&self) -> usize {
        self.entries.iter().map(|e| e.frame_depth).max().unwrap_or(0)
    }

    pub fn entries_for_opcode(&self, opcode: Opcode) -> Vec<&TraceEntry> {
        self.entries.iter().filter(|e| e.opcode == opcode).collect()
    }
}

pub(crate) struct TracerState {
    config: TraceConfig,
    entries: Vec<TraceEntry>,
    instruction_counter: usize,
    total_duration_ns: u128,
    limit_reached: bool,
}

impl TracerState {
    pub fn new(config: TraceConfig) -> Self {
        TracerState {
            config,
            entries: Vec::new(),
            instruction_counter: 0,
            total_duration_ns: 0,
            limit_reached: false,
        }
    }

    #[inline]
    pub fn should_record(&self, opcode: &Opcode) -> bool {
        if self.limit_reached {
            return false;
        }

        match &self.config.filter_opcodes {
            Some(filter) => filter.contains(opcode),
            None => true,
        }
    }

    #[inline]
    pub fn instruction_counter(&self) -> usize {
        self.instruction_counter
    }

    #[inline]
    pub fn should_capture_registers(&self) -> bool {
        self.config.capture_registers
    }

    #[allow(clippy::too_many_arguments)] // 追踪记录需要完整上下文（操作码/寄存器/IP/源码位置等），拆分会降低可读性
    pub fn record(
        &mut self,
        opcode: Opcode,
        operands: Vec<u16>,
        ip_before: usize,
        ip_after: usize,
        frame_depth: usize,
        registers_before: Option<Vec<Value>>,
        registers_after: Option<Vec<Value>>,
        duration_ns: u128,
        source_location: Option<SourceLocation>,
        function_name: Option<String>,
    ) {
        if let Some(max) = self.config.max_entries
            && self.entries.len() >= max
        {
            self.limit_reached = true;
            return;
        }

        let index = self.instruction_counter;
        self.instruction_counter += 1;

        self.total_duration_ns += duration_ns;

        let window = self.config.register_window.unwrap_or(DEFAULT_REGISTER_WINDOW);
        let regs_before = registers_before.map(|regs| Self::format_registers(&regs, window));
        let regs_after = registers_after.map(|regs| Self::format_registers(&regs, window));

        let entry = TraceEntry {
            instruction_index: index,
            opcode,
            operands,
            ip_before,
            ip_after,
            frame_depth,
            registers_before: regs_before,
            registers_after: regs_after,
            duration_ns,
            source_location,
            function_name,
        };

        self.entries.push(entry);
    }

    fn format_registers(registers: &[Value], window: usize) -> Vec<String> {
        registers.iter().take(window).map(Self::format_value).collect()
    }

    fn format_value(value: &Value) -> String {
        if value.is_nil() {
            "nil".to_string()
        } else if value.is_bool() {
            if value.as_bool() { "true" } else { "false" }.to_string()
        } else if value.is_number() {
            let num = value.as_number();
            if num.fract() == 0.0 && num.abs() < INTEGER_DISPLAY_THRESHOLD {
                format!("{}", num as i64)
            } else {
                format!("{:.2}", num)
            }
        } else if value.is_string() {
            let s = value.as_string_opt().unwrap_or_default();
            // ✅ 安全：使用 utf8_truncate 按字符数截断，避免多字节 UTF-8 字符 panic
            if s.chars().count() > STRING_PREVIEW_LEN {
                format!("\"{}...\"", utf8_truncate(&s, STRING_PREVIEW_KEEP))
            } else {
                format!("\"{}\"", s)
            }
        } else if value.is_closure() || value.is_builtin_fn() {
            "closure".to_string()
        } else if value.is_heap_object() {
            if let Some(heap_obj) = value.as_heap_object_opt() {
                match heap_obj.as_ref() {
                    nuzo_values::HeapObject::Array(arr) => {
                        format!("[...({})]", arr.len())
                    }
                    nuzo_values::HeapObject::Dict(dict) => {
                        format!("{{...({})}}", dict.len())
                    }
                    _ => "object".to_string(),
                }
            } else {
                "object".to_string()
            }
        } else {
            format!("{}", value)
        }
    }

    pub fn into_result(self, total_instructions: usize) -> TraceResult {
        TraceResult {
            entries: self.entries,
            total_instructions,
            total_duration_ns: self.total_duration_ns,
            config: self.config,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use nuzo_bytecode::Opcode;

    fn make_trace_result(entries: Vec<TraceEntry>, total: usize) -> TraceResult {
        TraceResult {
            entries,
            total_instructions: total,
            total_duration_ns: 0,
            config: TraceConfig::default(),
        }
    }

    fn make_entry(opcode: Opcode, frame_depth: usize) -> TraceEntry {
        TraceEntry {
            instruction_index: 0,
            opcode,
            operands: vec![],
            ip_before: 0,
            ip_after: 0,
            frame_depth,
            registers_before: None,
            registers_after: None,
            duration_ns: 0,
            source_location: None,
            function_name: None,
        }
    }

    #[test]
    fn test_filtered_count_basic() {
        let result = make_trace_result(vec![make_entry(Opcode::Halt, 0)], 10);
        assert_eq!(result.filtered_count(), 9);
    }

    #[test]
    fn test_filtered_count_zero() {
        let entries = vec![make_entry(Opcode::Halt, 0); 5];
        let result = make_trace_result(entries, 5);
        assert_eq!(result.filtered_count(), 0);
    }

    #[test]
    fn test_filtered_count_saturating() {
        let result = make_trace_result(vec![make_entry(Opcode::Halt, 0)], 0);
        assert_eq!(result.filtered_count(), 0);
    }

    #[test]
    fn test_format_trace_basic() {
        let result = make_trace_result(vec![], 0);
        let s = result.format_trace();
        assert!(s.contains("=== Trace Result ==="));
        assert!(s.contains("Total instructions: 0"));
    }

    #[test]
    fn test_format_trace_with_entries() {
        let entry = make_entry(Opcode::Halt, 0);
        let result = make_trace_result(vec![entry], 1);
        let s = result.format_trace();
        assert!(s.contains("Traced entries: 1"));
    }

    #[test]
    fn test_max_frame_depth_empty() {
        let result = make_trace_result(vec![], 0);
        assert_eq!(result.max_frame_depth(), 0);
    }

    #[test]
    fn test_max_frame_depth_basic() {
        let entries = vec![
            make_entry(Opcode::Halt, 1),
            make_entry(Opcode::Halt, 3),
            make_entry(Opcode::Halt, 2),
        ];
        let result = make_trace_result(entries, 3);
        assert_eq!(result.max_frame_depth(), 3);
    }

    #[test]
    fn test_tracer_state_instruction_counter_initial() {
        let state = TracerState::new(TraceConfig::default());
        assert_eq!(state.instruction_counter(), 0);
    }

    #[test]
    fn test_tracer_state_should_capture_registers_default() {
        let state = TracerState::new(TraceConfig::default());
        assert!(!state.should_capture_registers());
    }

    #[test]
    fn test_tracer_state_should_capture_registers_enabled() {
        let config = TraceConfig { capture_registers: true, ..TraceConfig::default() };
        let state = TracerState::new(config);
        assert!(state.should_capture_registers());
    }

    #[test]
    fn test_tracer_state_should_record_no_filter() {
        let state = TracerState::new(TraceConfig::default());
        assert!(state.should_record(&Opcode::Halt));
        assert!(state.should_record(&Opcode::Add));
    }

    #[test]
    fn test_tracer_state_should_record_with_filter() {
        let config =
            TraceConfig { filter_opcodes: Some(vec![Opcode::Halt]), ..TraceConfig::default() };
        let state = TracerState::new(config);
        assert!(state.should_record(&Opcode::Halt));
        assert!(!state.should_record(&Opcode::Add));
    }
}
