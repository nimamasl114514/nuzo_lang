macro_rules! emit_load_literal {
    ($self:ident, $value:expr, $line:expr) => {{
        let reg = $self.alloc_register()?;
        let const_idx = $self.add_constant_checked($value)?;
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::LoadK, $line);
        $self.emit_u16(reg);
        $self.emit_u16(const_idx);
        reg
    }};
}

macro_rules! emit_test_with_placeholder {
    ($self:ident, $reg:expr, $line:expr) => {{
        let ip = $self.chunk.code().len();
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Test, $line);
        $self.emit_u16($reg);
        $self.emit_i16(0);
        ip
    }};
}

macro_rules! emit_jmp_with_placeholder {
    ($self:ident, $line:expr) => {{
        let ip = $self.chunk.code().len();
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Jmp, $line);
        $self.emit_i16(0);
        ip
    }};
}

macro_rules! emit_load_nil {
    ($self:ident, $line:expr) => {{
        let reg = $self.alloc_register()?;
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::LoadNil, $line);
        $self.emit_u16(reg);
        reg
    }};
}

macro_rules! emit_binary_op {
    ($self:ident, $op:expr, $dest:expr, $left:expr, $right:expr, $line:expr) => {{
        $self.emit_opcode_with_line($op, $line);
        $self.emit_u16($dest);
        $self.emit_u16($left);
        $self.emit_u16($right);
    }};
}

macro_rules! emit_typed {
    ($self:expr, Halt) => {
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Halt, $self.current_line)
    };

    ($self:expr, LoadNil   , $reg:expr) => { emit_typed!(@reg1 $self, LoadNil  , $reg) };
    ($self:expr, LoadTrue  , $reg:expr) => { emit_typed!(@reg1 $self, LoadTrue , $reg) };
    ($self:expr, LoadFalse , $reg:expr) => { emit_typed!(@reg1 $self, LoadFalse, $reg) };
    ($self:expr, Print     , $reg:expr) => { emit_typed!(@reg1 $self, Print   , $reg) };
    ($self:expr, Return    , $reg:expr) => { emit_typed!(@reg1 $self, Return  , $reg) };

    ($self:expr, Mov         , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, Mov        , $a, $b) };
    ($self:expr, Neg         , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, Neg        , $a, $b) };
    ($self:expr, Not         , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, Not        , $a, $b) };
    ($self:expr, GetCaptured , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, GetCaptured, $a, $b) };
    ($self:expr, SetCaptured , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, SetCaptured, $a, $b) };
    ($self:expr, Len         , $a:expr, $b:expr) => { emit_typed!(@reg2 $self, Len        , $a, $b) };

    ($self:expr, Add     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Add     , $a, $b, $c) };
    ($self:expr, Sub     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Sub     , $a, $b, $c) };
    ($self:expr, Mul     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Mul     , $a, $b, $c) };
    ($self:expr, Div     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Div     , $a, $b, $c) };
    ($self:expr, Rem     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Rem     , $a, $b, $c) };
    ($self:expr, Mod     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Mod     , $a, $b, $c) };
    ($self:expr, Eq      , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Eq      , $a, $b, $c) };
    ($self:expr, Neq     , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Neq     , $a, $b, $c) };
    ($self:expr, Lt      , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Lt      , $a, $b, $c) };
    ($self:expr, Gt      , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Gt      , $a, $b, $c) };
    ($self:expr, Le      , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Le      , $a, $b, $c) };
    ($self:expr, Ge      , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Ge      , $a, $b, $c) };
    ($self:expr, GetIndex , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, GetIndex , $a, $b, $c) };
    ($self:expr, SetIndex , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, SetIndex , $a, $b, $c) };
    ($self:expr, SetIndexMut , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, SetIndexMut , $a, $b, $c) };
    ($self:expr, Capture  , $a:expr, $b:expr, $c:expr) => { emit_typed!(@reg3 $self, Capture  , $a, $b, $c) };

    ($self:expr, LoadK     , $reg:expr, $idx:expr) => { emit_typed!(@reg_const $self, LoadK    , $reg, $idx) };
    ($self:expr, Closure   , $reg:expr, $idx:expr) => { emit_typed!(@reg_const $self, Closure  , $reg, $idx) };
    ($self:expr, GetGlobal , $reg:expr, $idx:expr) => {{
        // ISS: GetGlobal 是 7 字节 (opcode + dest:u16 + name_idx:u16 + _iss_pad:u16)
        // 后 2 字节是 padding，运行时 patch 为 GetGlobalCached 的 version:u16
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::GetGlobal, $self.current_line);
        $self.emit_u16($reg);
        $self.emit_u16($idx);
        $self.emit_u16(0); // _iss_pad padding
    }};
    ($self:expr, SetGlobal , $reg:expr, $idx:expr) => { emit_typed!(@reg_const $self, SetGlobal, $reg, $idx) };

    ($self:expr, GetProp , $dest:expr, $obj:expr, $prop:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::GetProp, $self.current_line);
        $self.emit_u16($dest);
        $self.emit_u16($obj);
        $self.emit_u16($prop);
    }};
    ($self:expr, SetProp , $obj:expr, $prop:expr, $val:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::SetProp, $self.current_line);
        $self.emit_u16($obj);
        $self.emit_u16($prop);
        $self.emit_u16($val);
    }};

    ($self:expr, Jmp , $offset:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Jmp, $self.current_line);
        $self.emit_i16($offset);
    }};

    // 异常处理指令
    ($self:expr, TryStart , $offset:expr, $exc_reg:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::TryStart, $self.current_line);
        $self.emit_i16($offset);
        $self.emit_byte($exc_reg);
    }};
    ($self:expr, TryEnd) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::TryEnd, $self.current_line)
    }};
    ($self:expr, Out , $reg:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Out, $self.current_line);
        $self.emit_u16($reg);
    }};

    ($self:expr, Test , $reg:expr, $offset:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Test, $self.current_line);
        $self.emit_u16($reg);
        $self.emit_i16($offset);
    }};

    ($self:expr, Call     , $reg:expr, $u8_val:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::Call, $self.current_line);
        $self.emit_u16($reg);
        $self.emit_byte($u8_val);
    }};
    ($self:expr, ArrayNew , $reg:expr, $u16_val:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::ArrayNew, $self.current_line);
        $self.emit_u16($reg);
        $self.emit_u16($u16_val);
    }};
    ($self:expr, StringBuild , $dest:expr, $start:expr, $count:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::StringBuild, $self.current_line);
        $self.emit_u16($dest);
        $self.emit_u16($start);
        $self.emit_u16($count);
    }};

    ($self:expr, RangeNew , $dest:expr, $start:expr, $end:expr, $inc:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::RangeNew, $self.current_line);
        $self.emit_u16($dest);
        $self.emit_u16($start);
        $self.emit_u16($end);
        $self.emit_byte($inc);
    }};

    (@reg1 $self:expr, $op:ident, $reg:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::$op, $self.current_line);
        $self.emit_u16($reg);
    }};

    (@reg2 $self:expr, $op:ident, $a:expr, $b:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::$op, $self.current_line);
        $self.emit_u16($a);
        $self.emit_u16($b);
    }};

    (@reg3 $self:expr, $op:ident, $a:expr, $b:expr, $c:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::$op, $self.current_line);
        $self.emit_u16($a);
        $self.emit_u16($b);
        $self.emit_u16($c);
    }};

    (@reg_const $self:expr, $op:ident, $reg:expr, $idx:expr) => {{
        $self.emit_opcode_with_line(nuzo_bytecode::Opcode::$op, $self.current_line);
        $self.emit_u16($reg);
        $self.emit_u16($idx);
    }};
}

pub(crate) use emit_binary_op;
pub(crate) use emit_jmp_with_placeholder;
pub(crate) use emit_load_literal;
pub(crate) use emit_load_nil;
pub(crate) use emit_test_with_placeholder;
pub(crate) use emit_typed;
