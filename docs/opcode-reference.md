# Opcode Reference

> 自动生成，请勿手改。由 `nuzo_bytecode/build.rs` 从 `opcode.rs` 生成。

| Name | Code | Size | Description | Summary |
|------|------|------|-------------|--------|
| LoadK | 0 | 5 | 将常量池中的值加载到寄存器 | dest (Reg), constant_index (ConstIdx) |
| LoadNil | 1 | 3 | 将 nil 值加载到寄存器 | dest (Reg) |
| LoadTrue | 2 | 3 | 将 true 值加载到寄存器 | dest (Reg) |
| LoadFalse | 3 | 3 | 将 false 值加载到寄存器 | dest (Reg) |
| Mov | 4 | 5 | 将值从一个寄存器移动到另一个寄存器 | dest (Reg), src (Reg) |
| Add | 5 | 7 | 加法: dest = left + right | dest (Reg), left (Reg), right (Reg) |
| Sub | 6 | 7 | 减法: dest = left - right | dest (Reg), left (Reg), right (Reg) |
| Mul | 7 | 7 | 乘法: dest = left * right | dest (Reg), left (Reg), right (Reg) |
| Div | 8 | 7 | 除法: dest = left / right | dest (Reg), left (Reg), right (Reg) |
| Rem | 9 | 7 | 取余: dest = left % right | dest (Reg), left (Reg), right (Reg) |
| Neg | 10 | 5 | 取负: dest = -src | dest (Reg), src (Reg) |
| Eq | 11 | 7 | 相等比较: dest = (left == right) | dest (Reg), left (Reg), right (Reg) |
| Neq | 12 | 7 | 不等比较: dest = (left != right) | dest (Reg), left (Reg), right (Reg) |
| Lt | 13 | 7 | 小于比较: dest = (left < right) | dest (Reg), left (Reg), right (Reg) |
| Gt | 14 | 7 | 大于比较: dest = (left > right) | dest (Reg), left (Reg), right (Reg) |
| Le | 15 | 7 | 小于等于: dest = (left <= right) | dest (Reg), left (Reg), right (Reg) |
| Ge | 16 | 7 | 大于等于: dest = (left >= right) | dest (Reg), left (Reg), right (Reg) |
| Not | 17 | 5 | 逻辑非: dest = !src | dest (Reg), src (Reg) |
| Jmp | 18 | 3 | 无条件跳转 | offset (Offset) |
| Test | 19 | 5 | 条件跳转: 如果寄存器值为假则跳转 | reg (Reg), offset (Offset) |
| GetProp | 20 | 7 | 获取属性: dest = obj.property | dest (Reg), obj (Reg), prop (ConstIdx) |
| SetProp | 21 | 7 | 设置属性: obj.property = value | obj (Reg), prop (ConstIdx), val (Reg) |
| GetIndex | 22 | 7 | 获取索引: dest = obj[index] | dest (Reg), left (Reg), right (Reg) |
| SetIndex | 23 | 7 | 设置索引: obj[index] = value | obj (Reg), index (Reg), val (Reg) |
| SetIndexMut | 41 | 7 | 设置索引（原地修改）: obj[index] = value | obj (Reg), index (Reg), val (Reg) |
| Call | 24 | 4 | 调用函数 | func_reg (Reg), argc (U8) |
| Return | 25 | 3 | 函数返回 | dest (Reg) |
| Closure | 26 | 5 | 创建闭包 | dest (Reg), constant_index (ConstIdx) |
| Print | 27 | 3 | 打印寄存器值 | dest (Reg) |
| Halt | 28 | 1 | 停止虚拟机 | (undocumented) |
| ArrayNew | 29 | 5 | 创建新数组 | dest (Reg), count (U16) |
| InitModule | 30 | 5 | 初始化模块(lazy import) | module_idx (ConstIdx), init_flag_slot (U16) |
| Capture | 31 | 7 | 捕获变量到闭包 | closure (Reg), capture_idx (CaptureIdx), source (Reg/u16) |
| GetCaptured | 32 | 5 | 从闭包获取捕获的变量 | dest (Reg), capture_idx (CaptureIdx) |
| SetCaptured | 33 | 5 | 设置闭包中的捕获变量 | capture_idx (CaptureIdx), val (Reg) |
| GetGlobal | 35 | 7 | 获取全局变量(ISS预留缓存空间) | dest (Reg), name_idx (ConstIdx), _iss_gidx (U16) |
| SetGlobal | 36 | 5 | 设置全局变量 | dest (Reg), constant_index (ConstIdx) |
| RangeNew | 37 | 8 | 创建范围对象: dest = start..end (含 inclusive 标志) | dest (Reg), start (Reg), end (Reg), inclusive (U8) |
| Mod | 38 | 7 | 取模: dest = left % right | dest (Reg), left (Reg), right (Reg) |
| Len | 39 | 5 | 获取长度: dest = len(obj) | dest (Reg), src (Reg) |
| Pow | 40 | 7 | 幂运算: dest = base ^ exp | dest (Reg), left (Reg), right (Reg) |
| GetGlobalCached | 50 | 7 | ISS特化全局变量读取(内联缓存) | dest (Reg), global_idx (U16), version (U16) |
| TryStart | 51 | 4 | 标记try块开始，记录catch跳转目标 | catch_offset (Offset), exception_reg (U8) |
| TryEnd | 52 | 1 | 标记try块结束（正常路径） | (undocumented) |
| Out | 53 | 3 | 抛出异常(out语句) | value_reg (Reg) |
| SpillLoad | 54 | 5 | LSRA Spill 加载: 从 spill_stack[slot] 加载到 R[dst] | dst (Reg), slot (U16) |
| SpillStore | 55 | 5 | LSRA Spill 存储: 从 R[src] 存储到 spill_stack[slot] | src (Reg), slot (U16) |
