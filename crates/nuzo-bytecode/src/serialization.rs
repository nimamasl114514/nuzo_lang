//! Bytecode file serialization with version validation.
//!
//! This module defines a versioned bytecode file format for standalone `.nuzo`
//! files. The format stores the complete [`Chunk`] required for execution:
//!
//! ```text
//! | magic (4) | version (4) | code_len (4) | code (code_len) |
//! | constants_len (4) | constants (*) |
//! | lines_len (4) | lines (lines_len * 4) |
//! | debug_info_len (4) | debug_info (debug_info_len) |
//! | locals_count (2) | spill_slot_count (2) |
//! ```
//!
//! All multi-byte fields are little-endian. The header (magic, version, and
//! code block position) is frozen so that older loaders can at least detect
//! version mismatches and reject files cleanly.
//!
//! Constants are serialized as a one-byte tag followed by a type-specific
//! payload. Currently supported constant types are `Nil`, `Bool`, `Smi`,
//! `Float`, and `String`. Attempting to serialize or deserialize any other
//! value type yields a clear [`NuzoError`].

use crate::Chunk;
use nuzo_core::error::ErrorCode;
use nuzo_core::{InternalError, NuzoError, Value, ValueTag};
use nuzo_values::{DeadCodeReason, DeadCodeRecord, DebugInfo, FoldRecord, InlineRecord, ValueExt};
use std::sync::Arc;

/// Bytecode file magic number: `b"NUZO"`.
pub const BYTECODE_MAGIC: &[u8; 4] = b"NUZO";

/// Current bytecode file format version.
///
/// Bump this only when the file format itself changes (not when opcodes are
/// appended). Bumping this version will cause older files to be rejected with
/// [`InternalError::InvalidBytecodeVersion`].
pub const BYTECODE_VERSION: u32 = 2;

/// Minimum bytes required to read the code block length: magic (4) + version (4) + code_len (4).
const HEADER_SIZE: usize = 12;

// ── constant value serialization tags ───────────────────────────────────────

const VALUE_TAG_NIL: u8 = 0;
const VALUE_TAG_BOOL: u8 = 1;
const VALUE_TAG_SMI: u8 = 2;
const VALUE_TAG_FLOAT: u8 = 3;
const VALUE_TAG_STRING: u8 = 4;

// ── dead-code reason serialization tags ─────────────────────────────────────

const DEAD_CODE_UNREACHABLE: u8 = 0;
const DEAD_CODE_UNUSED_VAR: u8 = 1;
const DEAD_CODE_CONSTANT_COND: u8 = 2;
const DEAD_CODE_OTHER: u8 = 3;

/// Serialize a complete [`Chunk`] into the versioned bytecode format.
///
/// # Errors
///
/// Returns [`NuzoError::internal`] with [`InternalError::CompilerBug`] if the
/// chunk contains a constant value type that cannot currently be serialized.
pub fn save_chunk(chunk: &Chunk) -> Result<Vec<u8>, NuzoError> {
    let mut out = Vec::with_capacity(HEADER_SIZE + chunk.len());

    // Header
    out.extend_from_slice(BYTECODE_MAGIC);
    out.extend_from_slice(&BYTECODE_VERSION.to_le_bytes());

    // Code block
    let code = chunk.code();
    if code.len() > u32::MAX as usize {
        // 字节码长度超过 u32 上限会静默截断，导致 load_chunk 读取错误长度。
        // 显式报错优于静默截断。
        return Err(NuzoError::internal(
            InternalError::CompilerBug {
                message: format!(
                    "bytecode code block length {} exceeds u32::MAX ({}); \
                     cannot serialize without truncation",
                    code.len(),
                    u32::MAX,
                ),
            },
            None,
        ));
    }
    write_u32(&mut out, code.len() as u32);
    out.extend_from_slice(code);

    // Constants pool
    let constants = chunk.constants();
    write_u32(&mut out, constants.len() as u32);
    for &value in constants {
        out.extend_from_slice(&serialize_value(value)?);
    }

    // Line table
    let lines = chunk.lines();
    write_u32(&mut out, lines.len() as u32);
    for &line in lines {
        write_u32(&mut out, line);
    }

    // Debug info
    let debug_bytes = serialize_debug_info(&chunk.debug_info);
    write_u32(&mut out, debug_bytes.len() as u32);
    out.extend_from_slice(&debug_bytes);

    // Frame/stack metadata
    out.extend_from_slice(&chunk.locals_count.to_le_bytes());
    out.extend_from_slice(&chunk.spill_slot_count.to_le_bytes());

    Ok(out)
}

/// Load a complete [`Chunk`] from the versioned bytecode format, validating
/// magic and version before reconstructing the instruction stream, constants,
/// debug info, and frame metadata.
///
/// # Errors
///
/// Returns [`NuzoError::internal`] with:
/// - [`InternalError::InvalidBytecodeVersion`] if the magic is wrong or the
///   file version does not match [`BYTECODE_VERSION`].
/// - [`InternalError::BytecodeOutOfBounds`] if any length field points past
///   the end of the buffer.
/// - [`InternalError::CompilerBug`] if a constant uses an unsupported type tag.
pub fn load_chunk(bytes: &[u8]) -> Result<Chunk, NuzoError> {
    if bytes.len() < HEADER_SIZE {
        return Err(NuzoError::internal(
            InternalError::InvalidBytecodeVersion {
                expected: BYTECODE_VERSION,
                got: 0,
                opcode: None,
            },
            None,
        )
        .with_code(ErrorCode::InvalidBytecodeVersion));
    }

    if &bytes[0..4] != BYTECODE_MAGIC.as_slice() {
        return Err(NuzoError::internal(
            InternalError::InvalidBytecodeVersion {
                expected: BYTECODE_VERSION,
                got: 0,
                opcode: None,
            },
            None,
        )
        .with_code(ErrorCode::InvalidBytecodeVersion));
    }

    let mut pos = 4;
    let got_version = read_u32(bytes, &mut pos)?;
    if got_version != BYTECODE_VERSION {
        return Err(NuzoError::internal(
            InternalError::InvalidBytecodeVersion {
                expected: BYTECODE_VERSION,
                got: got_version,
                opcode: None,
            },
            None,
        )
        .with_code(ErrorCode::InvalidBytecodeVersion));
    }

    // Code block
    let code_len = read_u32(bytes, &mut pos)? as usize;
    let code_end = pos.checked_add(code_len).ok_or_else(|| out_of_bounds(pos, bytes.len()))?;
    if bytes.len() < code_end {
        return Err(out_of_bounds(pos + code_len.saturating_sub(1), bytes.len()));
    }
    let code = bytes[pos..code_end].to_vec();
    pos = code_end;

    // Constants pool
    let constants_len = read_u32(bytes, &mut pos)? as usize;
    let mut constants = Vec::with_capacity(constants_len);
    for _ in 0..constants_len {
        constants.push(deserialize_value(bytes, &mut pos)?);
    }

    // Line table
    let lines_len = read_u32(bytes, &mut pos)? as usize;
    let mut lines = Vec::with_capacity(lines_len);
    for _ in 0..lines_len {
        lines.push(read_u32(bytes, &mut pos)?);
    }

    // Debug info
    let debug_len = read_u32(bytes, &mut pos)? as usize;
    let debug_end = pos.checked_add(debug_len).ok_or_else(|| out_of_bounds(pos, bytes.len()))?;
    if bytes.len() < debug_end {
        return Err(out_of_bounds(pos + debug_len.saturating_sub(1), bytes.len()));
    }
    let debug_info = deserialize_debug_info(&bytes[pos..debug_end])?;
    pos = debug_end;

    // Frame/stack metadata
    let locals_count = read_u16(bytes, &mut pos)?;
    let spill_slot_count = read_u16(bytes, &mut pos)?;

    Ok(Chunk::from_arcs(
        Arc::new(code),
        Arc::new(constants),
        Arc::new(lines),
        Arc::new(debug_info),
        locals_count,
        spill_slot_count,
    ))
}

/// Deprecated alias for [`save_chunk`].
#[deprecated(
    since = "0.5.0",
    note = "use save_chunk which serializes the full Chunk; will be removed in 0.7.0"
)]
pub fn save_chunk_code(chunk: &Chunk) -> Result<Vec<u8>, NuzoError> {
    save_chunk(chunk)
}

/// Deprecated alias for [`load_chunk`].
#[deprecated(
    since = "0.5.0",
    note = "use load_chunk which deserializes the full Chunk; will be removed in 0.7.0"
)]
pub fn load_chunk_code(bytes: &[u8]) -> Result<Chunk, NuzoError> {
    load_chunk(bytes)
}

/// Diagnose a raw opcode byte.
///
/// If the byte does not correspond to a known [`Opcode`](crate::Opcode),
/// returns a [`NuzoError`] carrying [`InternalError::InvalidOpcode`] so that
/// callers can surface a structured diagnostic instead of a bare `None`.
///
/// Returns `None` for valid opcode bytes.
pub fn diagnose_opcode_byte(byte: u8) -> Option<NuzoError> {
    if Chunk::decode_opcode(byte).is_none() {
        Some(NuzoError::internal(InternalError::InvalidOpcode { opcode: byte }, None))
    } else {
        None
    }
}

// ── low-level primitive helpers ─────────────────────────────────────────────

fn write_u32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, val: u64) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_string(out: &mut Vec<u8>, s: &str) {
    write_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

fn read_u32(bytes: &[u8], pos: &mut usize) -> Result<u32, NuzoError> {
    if bytes.len() < *pos + 4 {
        return Err(out_of_bounds(*pos, bytes.len()));
    }
    let val = u32::from_le_bytes([bytes[*pos], bytes[*pos + 1], bytes[*pos + 2], bytes[*pos + 3]]);
    *pos += 4;
    Ok(val)
}

fn read_u16(bytes: &[u8], pos: &mut usize) -> Result<u16, NuzoError> {
    if bytes.len() < *pos + 2 {
        return Err(out_of_bounds(*pos, bytes.len()));
    }
    let val = u16::from_le_bytes([bytes[*pos], bytes[*pos + 1]]);
    *pos += 2;
    Ok(val)
}

fn read_u64(bytes: &[u8], pos: &mut usize) -> Result<u64, NuzoError> {
    if bytes.len() < *pos + 8 {
        return Err(out_of_bounds(*pos, bytes.len()));
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[*pos..*pos + 8]);
    *pos += 8;
    Ok(u64::from_le_bytes(buf))
}

fn read_i64(bytes: &[u8], pos: &mut usize) -> Result<i64, NuzoError> {
    Ok(read_u64(bytes, pos)? as i64)
}

fn read_byte(bytes: &[u8], pos: &mut usize) -> Result<u8, NuzoError> {
    if bytes.len() <= *pos {
        return Err(out_of_bounds(*pos, bytes.len()));
    }
    let b = bytes[*pos];
    *pos += 1;
    Ok(b)
}

fn read_string(bytes: &[u8], pos: &mut usize) -> Result<String, NuzoError> {
    let len = read_u32(bytes, pos)? as usize;
    let end = pos.checked_add(len).ok_or_else(|| out_of_bounds(*pos, bytes.len()))?;
    if bytes.len() < end {
        return Err(out_of_bounds(*pos + len.saturating_sub(1), bytes.len()));
    }
    let s = String::from_utf8(bytes[*pos..end].to_vec()).map_err(|_| {
        NuzoError::internal(
            InternalError::CompilerBug {
                message: "bytecode debug info contains invalid UTF-8".to_string(),
            },
            None,
        )
    })?;
    *pos = end;
    Ok(s)
}

fn out_of_bounds(ip: usize, code_len: usize) -> NuzoError {
    NuzoError::internal(InternalError::BytecodeOutOfBounds { ip, code_len }, None)
}

// ── Value serialization ─────────────────────────────────────────────────────

fn serialize_value(value: Value) -> Result<Vec<u8>, NuzoError> {
    let mut out = Vec::new();
    match value.tag() {
        ValueTag::Nil => out.push(VALUE_TAG_NIL),
        ValueTag::Bool => {
            out.push(VALUE_TAG_BOOL);
            out.push(if value.as_bool() { 1 } else { 0 });
        }
        ValueTag::Smi => {
            out.push(VALUE_TAG_SMI);
            write_u64(&mut out, value.as_smi() as u64);
        }
        ValueTag::Float => {
            out.push(VALUE_TAG_FLOAT);
            write_u64(&mut out, value.into_raw_bits());
        }
        ValueTag::String => {
            out.push(VALUE_TAG_STRING);
            let s = value.as_string_opt().ok_or_else(|| unsupported_value(ValueTag::String))?;
            write_string(&mut out, &s);
        }
        other => return Err(unsupported_value(other)),
    }
    Ok(out)
}

fn deserialize_value(bytes: &[u8], pos: &mut usize) -> Result<Value, NuzoError> {
    let tag = read_byte(bytes, pos)?;
    match tag {
        VALUE_TAG_NIL => Ok(nuzo_values::NIL),
        VALUE_TAG_BOOL => Ok(Value::from_bool(read_byte(bytes, pos)? != 0)),
        VALUE_TAG_SMI => {
            let i = read_i64(bytes, pos)?;
            Value::try_from_smi(i).ok_or_else(|| {
                NuzoError::internal(
                    InternalError::CompilerBug {
                        message: format!("serialized Smi value {} is out of range", i),
                    },
                    None,
                )
            })
        }
        VALUE_TAG_FLOAT => Ok(Value::from_bits(read_u64(bytes, pos)?)),
        VALUE_TAG_STRING => {
            let s = read_string(bytes, pos)?;
            Ok(<Value as ValueExt>::from_string(&s))
        }
        _ => Err(NuzoError::internal(
            InternalError::CompilerBug {
                message: format!("unsupported constant value tag in bytecode: {}", tag),
            },
            None,
        )),
    }
}

fn unsupported_value(tag: ValueTag) -> NuzoError {
    NuzoError::internal(
        InternalError::CompilerBug {
            message: format!(
                "unsupported constant type for bytecode serialization: {}. \
                 Supported types are Nil, Bool, Smi, Float, and String.",
                tag
            ),
        },
        None,
    )
}

// ── DebugInfo serialization ─────────────────────────────────────────────────

fn serialize_debug_info(info: &DebugInfo) -> Vec<u8> {
    let mut out = Vec::new();

    write_string(&mut out, &info.source_file);

    write_u32(&mut out, info.source_lines.len() as u32);
    for line in &info.source_lines {
        write_string(&mut out, line);
    }

    write_u32(&mut out, info.ip_to_line.len() as u32);
    for (&ip, &line) in &info.ip_to_line {
        write_u64(&mut out, ip as u64);
        write_u64(&mut out, line as u64);
    }

    write_u32(&mut out, info.ip_to_column.len() as u32);
    for (&ip, &col) in &info.ip_to_column {
        write_u64(&mut out, ip as u64);
        write_u64(&mut out, col as u64);
    }

    match &info.function_name {
        Some(name) => {
            out.push(1);
            write_string(&mut out, name);
        }
        None => out.push(0),
    }

    write_u32(&mut out, info.inline_records.len() as u32);
    for record in &info.inline_records {
        write_u64(&mut out, record.ip_start as u64);
        write_u64(&mut out, record.ip_end as u64);
        write_string(&mut out, &record.function_name);
        write_string(&mut out, &record.source_file);
        write_u64(&mut out, record.call_site_line as u64);
    }

    write_u32(&mut out, info.dead_code_records.len() as u32);
    for record in &info.dead_code_records {
        write_u64(&mut out, record.source_line_start as u64);
        write_u64(&mut out, record.source_line_end as u64);
        serialize_dead_code_reason(&mut out, &record.reason);
    }

    write_u32(&mut out, info.fold_records.len() as u32);
    for record in &info.fold_records {
        write_u64(&mut out, record.result_const_idx as u64);
        write_u64(&mut out, record.ip as u64);
        write_string(&mut out, &record.description);
        write_u64(&mut out, record.source_line as u64);
    }

    out
}

fn serialize_dead_code_reason(out: &mut Vec<u8>, reason: &DeadCodeReason) {
    match reason {
        DeadCodeReason::UnreachableCode => out.push(DEAD_CODE_UNREACHABLE),
        DeadCodeReason::UnusedVariable(name) => {
            out.push(DEAD_CODE_UNUSED_VAR);
            write_string(out, name);
        }
        DeadCodeReason::ConstantCondition(cond) => {
            out.push(DEAD_CODE_CONSTANT_COND);
            out.push(if *cond { 1 } else { 0 });
        }
        DeadCodeReason::Other(text) => {
            out.push(DEAD_CODE_OTHER);
            write_string(out, text);
        }
    }
}

fn deserialize_debug_info(bytes: &[u8]) -> Result<DebugInfo, NuzoError> {
    let mut pos = 0;

    let source_file = read_string(bytes, &mut pos)?;

    let source_lines_len = read_u32(bytes, &mut pos)? as usize;
    let mut source_lines = Vec::with_capacity(source_lines_len);
    for _ in 0..source_lines_len {
        source_lines.push(read_string(bytes, &mut pos)?);
    }

    let ip_to_line_len = read_u32(bytes, &mut pos)? as usize;
    let mut ip_to_line = nuzo_core::XxHashMap::default();
    for _ in 0..ip_to_line_len {
        let ip = read_u64(bytes, &mut pos)? as usize;
        let line = read_u64(bytes, &mut pos)? as usize;
        ip_to_line.insert(ip, line);
    }

    let ip_to_column_len = read_u32(bytes, &mut pos)? as usize;
    let mut ip_to_column = nuzo_core::XxHashMap::default();
    for _ in 0..ip_to_column_len {
        let ip = read_u64(bytes, &mut pos)? as usize;
        let col = read_u64(bytes, &mut pos)? as usize;
        ip_to_column.insert(ip, col);
    }

    let function_name =
        if read_byte(bytes, &mut pos)? != 0 { Some(read_string(bytes, &mut pos)?) } else { None };

    let inline_len = read_u32(bytes, &mut pos)? as usize;
    let mut inline_records = Vec::with_capacity(inline_len);
    for _ in 0..inline_len {
        inline_records.push(InlineRecord {
            ip_start: read_u64(bytes, &mut pos)? as usize,
            ip_end: read_u64(bytes, &mut pos)? as usize,
            function_name: read_string(bytes, &mut pos)?,
            source_file: read_string(bytes, &mut pos)?,
            call_site_line: read_u64(bytes, &mut pos)? as usize,
        });
    }

    let dead_len = read_u32(bytes, &mut pos)? as usize;
    let mut dead_code_records = Vec::with_capacity(dead_len);
    for _ in 0..dead_len {
        dead_code_records.push(DeadCodeRecord {
            source_line_start: read_u64(bytes, &mut pos)? as usize,
            source_line_end: read_u64(bytes, &mut pos)? as usize,
            reason: deserialize_dead_code_reason(bytes, &mut pos)?,
        });
    }

    let fold_len = read_u32(bytes, &mut pos)? as usize;
    let mut fold_records = Vec::with_capacity(fold_len);
    for _ in 0..fold_len {
        fold_records.push(FoldRecord {
            result_const_idx: read_u64(bytes, &mut pos)? as usize,
            ip: read_u64(bytes, &mut pos)? as usize,
            description: read_string(bytes, &mut pos)?,
            source_line: read_u64(bytes, &mut pos)? as usize,
        });
    }

    Ok(DebugInfo {
        source_file,
        source_lines,
        ip_to_line,
        ip_to_column,
        function_name,
        inline_records,
        dead_code_records,
        fold_records,
    })
}

fn deserialize_dead_code_reason(
    bytes: &[u8],
    pos: &mut usize,
) -> Result<DeadCodeReason, NuzoError> {
    let tag = read_byte(bytes, pos)?;
    match tag {
        DEAD_CODE_UNREACHABLE => Ok(DeadCodeReason::UnreachableCode),
        DEAD_CODE_UNUSED_VAR => Ok(DeadCodeReason::UnusedVariable(read_string(bytes, pos)?)),
        DEAD_CODE_CONSTANT_COND => {
            Ok(DeadCodeReason::ConstantCondition(read_byte(bytes, pos)? != 0))
        }
        DEAD_CODE_OTHER => Ok(DeadCodeReason::Other(read_string(bytes, pos)?)),
        _ => Err(NuzoError::internal(
            InternalError::CompilerBug {
                message: format!("unsupported dead-code reason tag in bytecode: {}", tag),
            },
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Chunk, ConstIdx, Opcode, Reg};
    use nuzo_values::{FALSE, NIL, TRUE};

    #[test]
    fn test_bytecode_version_mismatch() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Halt);
        let mut bytes = save_chunk(&chunk).expect("save should succeed");

        // Corrupt the version field (offset 4..8) to a future version.
        bytes[4..8].copy_from_slice(&999_u32.to_le_bytes());

        let err = load_chunk(&bytes).expect_err("mismatched version should fail");
        match &err.kind {
            nuzo_core::NuzoErrorKind::Internal(
                InternalError::InvalidBytecodeVersion { expected, got, opcode },
                _,
            ) => {
                assert_eq!(*expected, BYTECODE_VERSION);
                assert_eq!(*got, 999);
                assert_eq!(*opcode, None);
            }
            other => panic!("expected InvalidBytecodeVersion, got {:?}", other),
        }
        assert_eq!(err.code(), ErrorCode::InvalidBytecodeVersion);
        let msg = format!("{}", err);
        assert!(
            msg.contains("expected version 2"),
            "message should report expected version: {}",
            msg
        );
        assert!(msg.contains("got version 999"), "message should report actual version: {}", msg);
    }

    #[test]
    fn test_old_version_1_rejected() {
        // A minimal version-1 file: magic + version 1 + empty code block.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(BYTECODE_MAGIC);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let err = load_chunk(&bytes).expect_err("version 1 should fail");
        match &err.kind {
            nuzo_core::NuzoErrorKind::Internal(
                InternalError::InvalidBytecodeVersion { expected, got, opcode },
                _,
            ) => {
                assert_eq!(*expected, BYTECODE_VERSION);
                assert_eq!(*got, 1);
                assert_eq!(*opcode, None);
            }
            other => panic!("expected InvalidBytecodeVersion, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_opcode_diagnosis() {
        let bad_opcode: u8 = 0xFF;
        assert!(Chunk::decode_opcode(bad_opcode).is_none(), "0xFF should not be a valid opcode");

        let err = diagnose_opcode_byte(bad_opcode).expect("unknown opcode should produce an error");
        match &err.kind {
            nuzo_core::NuzoErrorKind::Internal(InternalError::InvalidOpcode { opcode }, _) => {
                assert_eq!(*opcode, bad_opcode);
            }
            other => panic!("expected InvalidOpcode, got {:?}", other),
        }
        let msg = format!("{}", err);
        assert!(msg.contains("invalid opcode 0xFF"), "message should report opcode: {}", msg);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::LoadNil);
        chunk.write_u16(0);
        chunk.write_opcode(Opcode::Halt);

        let bytes = save_chunk(&chunk).expect("save should succeed");
        let loaded = load_chunk(&bytes).expect("roundtrip should succeed");
        assert_eq!(loaded.code(), chunk.code());
        assert!(loaded.constants().is_empty());
        assert!(loaded.lines().is_empty());
        assert_eq!(loaded.locals_count, 0);
        assert_eq!(loaded.spill_slot_count, 0);
    }

    #[test]
    fn test_invalid_magic_rejected() {
        let mut bytes = save_chunk(&Chunk::new()).expect("save should succeed");
        bytes[0] = b'X';
        let err = load_chunk(&bytes).expect_err("bad magic should fail");
        assert!(
            matches!(
                &err.kind,
                nuzo_core::NuzoErrorKind::Internal(InternalError::InvalidBytecodeVersion { .. }, _)
            ),
            "expected InvalidBytecodeVersion, got {:?}",
            err.kind
        );
    }

    #[test]
    fn test_truncated_code_rejected() {
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Halt);
        let mut bytes = save_chunk(&chunk).expect("save should succeed");
        // Claim a larger code block than present.
        bytes[8..12].copy_from_slice(&100_u32.to_le_bytes());
        let err = load_chunk(&bytes).expect_err("truncated code should fail");
        assert!(
            matches!(
                &err.kind,
                nuzo_core::NuzoErrorKind::Internal(InternalError::BytecodeOutOfBounds { .. }, _)
            ),
            "expected BytecodeOutOfBounds, got {:?}",
            err.kind
        );
    }

    #[test]
    fn test_full_chunk_roundtrip() {
        let mut chunk = Chunk::new();
        chunk.locals_count = 7;
        chunk.spill_slot_count = 3;

        // Constants covering all supported types.
        let c_nil = chunk.add_constant(NIL);
        let c_true = chunk.add_constant(TRUE);
        let c_false = chunk.add_constant(FALSE);
        let c_smi = chunk.add_constant(Value::from_smi(42));
        let c_float = chunk.add_constant(Value::from_number(4.2));
        let c_string = chunk.add_constant(<Value as ValueExt>::from_string("hello nuzo"));

        // Instructions that reference the constants.
        chunk.emit(crate::Instruction::LoadK { dest: Reg(0), const_idx: ConstIdx(c_nil as u16) });
        chunk.emit(crate::Instruction::LoadK { dest: Reg(1), const_idx: ConstIdx(c_true as u16) });
        chunk.emit(crate::Instruction::LoadK { dest: Reg(2), const_idx: ConstIdx(c_false as u16) });
        chunk.emit(crate::Instruction::LoadK { dest: Reg(3), const_idx: ConstIdx(c_smi as u16) });
        chunk.emit(crate::Instruction::LoadK { dest: Reg(4), const_idx: ConstIdx(c_float as u16) });
        chunk
            .emit(crate::Instruction::LoadK { dest: Reg(5), const_idx: ConstIdx(c_string as u16) });
        chunk.emit(crate::Instruction::Halt);

        // Line table.
        chunk.lines_mut().extend_from_slice(&[1, 1, 2, 2, 3, 3, 4]);

        // Debug info.
        {
            let debug = Arc::make_mut(&mut chunk.debug_info);
            debug.source_file = "test.nuzo".to_string();
            debug.source_lines.push("let x = 1;".to_string());
            debug.source_lines.push("let y = 2;".to_string());
            debug.ip_to_line.insert(0, 1);
            debug.ip_to_line.insert(5, 2);
            debug.ip_to_column.insert(5, 4);
            debug.function_name = Some("test_fn".to_string());
        }

        let debug_bytes = serialize_debug_info(&chunk.debug_info);
        eprintln!("DEBUG bytes len={} {:?}", debug_bytes.len(), debug_bytes);
        let bytes = save_chunk(&chunk).expect("save full chunk should succeed");
        eprintln!("FULL bytes len={} {:?}", bytes.len(), bytes);
        let loaded = load_chunk(&bytes).expect("roundtrip should succeed");

        assert_eq!(loaded.code(), chunk.code());
        assert_eq!(loaded.constants(), chunk.constants());
        assert_eq!(loaded.lines(), chunk.lines());
        assert_eq!(loaded.locals_count, chunk.locals_count);
        assert_eq!(loaded.spill_slot_count, chunk.spill_slot_count);

        let debug = &loaded.debug_info;
        assert_eq!(debug.source_file, "test.nuzo");
        assert_eq!(debug.source_lines, vec!["let x = 1;", "let y = 2;"]);
        assert_eq!(debug.ip_to_line.get(&0), Some(&1));
        assert_eq!(debug.ip_to_line.get(&5), Some(&2));
        assert_eq!(debug.ip_to_column.get(&5), Some(&4));
        assert_eq!(debug.function_name, Some("test_fn".to_string()));
    }

    #[test]
    fn test_unsupported_value_tag_rejected() {
        // Craft a constants block with one constant and an unknown tag.
        let mut chunk = Chunk::new();
        chunk.write_opcode(Opcode::Halt);
        let mut bytes = save_chunk(&chunk).expect("save should succeed");

        // Locate the constants_len field: it starts right after the code block.
        // Code block length is 1, so constants_len starts at HEADER_SIZE + 1 = 13.
        let constants_offset = HEADER_SIZE + 1;
        // Overwrite constants_len from 0 to 1.
        bytes[constants_offset..constants_offset + 4].copy_from_slice(&1u32.to_le_bytes());
        // Insert a constant with an unsupported tag immediately after constants_len.
        bytes.insert(constants_offset + 4, 0xFF);

        let err = load_chunk(&bytes).expect_err("unsupported value tag should fail");
        assert!(
            matches!(
                &err.kind,
                nuzo_core::NuzoErrorKind::Internal(InternalError::CompilerBug { .. }, _)
            ),
            "expected CompilerBug for unsupported constant, got {:?}",
            err.kind
        );
        let msg = format!("{}", err);
        assert!(
            msg.contains("unsupported constant value tag"),
            "message should mention unsupported tag: {}",
            msg
        );
    }

    #[test]
    fn test_unsupported_constant_type_fails_to_save() {
        let mut chunk = Chunk::new();
        // A raw pointer value is not a supported constant type.
        let ptr_value = unsafe { Value::from_ptr(std::ptr::null()) };
        chunk.add_constant(ptr_value);

        let err = save_chunk(&chunk).expect_err("saving unsupported constant should fail");
        assert!(
            matches!(
                &err.kind,
                nuzo_core::NuzoErrorKind::Internal(InternalError::CompilerBug { .. }, _)
            ),
            "expected CompilerBug for unsupported constant, got {:?}",
            err.kind
        );
        let msg = format!("{}", err);
        assert!(
            msg.contains("unsupported constant type"),
            "message should mention unsupported type: {}",
            msg
        );
    }
}
