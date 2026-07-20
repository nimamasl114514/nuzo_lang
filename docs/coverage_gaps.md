# 测试覆盖率缺口报告

> 自动生成于 2026-06-20 01:41:47 UTC | 数据源: CALL_GRAPH.md

## 概览

- CALL_GRAPH pub fn 总数: **1252**
- 已有测试函数总数: **2470**
- 已覆盖 pub fn: **67**
- 未覆盖 pub fn: **1185**
- 覆盖率: **5.4%**

## 未覆盖 pub fn 明细

| 模块 | 函数路径 | 行号 | 签名 |
|------|---------|------|------|
| allocator | Interval::is_empty | 991-993 | pub fn is_empty(&self) -> bool |
| allocator | Interval::len | 985-987 | pub fn len(&self) -> usize |
| allocator | Interval::new | 965-981 | pub fn new(vreg: u16, start: usize, end: usize) -> Self |
| allocator | Interval::overlaps | 1000-1002 | pub fn overlaps(&self, other: &Self) -> bool |
| allocator | LsraAllocator::active_count | 1444-1446 | pub fn active_count(&self) -> usize |
| allocator | LsraAllocator::alloc_one | 1370-1392 | pub fn alloc_one(&mut self) -> Option |
| allocator | LsraAllocator::allocate | 1519-1523 | pub fn allocate(&mut self, intervals: &mut [Interval]) ->... |
| allocator | LsraAllocator::expire_old_intervals | 1475-1502 | pub fn expire_old_intervals(&mut self, current_pos: usize) |
| allocator | LsraAllocator::free | 1404-1424 | pub fn free(&mut self, reg: u16) |
| allocator | LsraAllocator::free_count | 1438-1440 | pub fn free_count(&self) -> u16 |
| allocator | LsraAllocator::get_phys_reg | 1730-1736 | pub fn get_phys_reg(&self, vreg: u16) -> Option |
| allocator | LsraAllocator::handled_intervals | 1740-1742 | pub fn handled_intervals(&self) -> &[Interval] |
| allocator | LsraAllocator::has_free | 1430-1432 | pub fn has_free(&self) -> bool |
| allocator | LsraAllocator::new | 1279-1309 | pub fn new(max_regs: u16) -> Self |
| allocator | LsraAllocator::reset | 1753-1770 | pub fn reset(&mut self) |
| allocator | LsraAllocator::spill_slot_count | 1746-1748 | pub fn spill_slot_count(&self) -> u16 |
| allocator | LsraAllocator::with_max_locals | 1312-1314 | pub fn with_max_locals() -> Self |
| allocator | LsraAllocator::with_nud_config | 1324-1352 | pub fn with_nud_config(nud_config: NudConfig) -> Self |
| allocator | NudConfig::disabled | 1122-1127 | pub fn disabled() -> Self |
| allocator | NudConfig::effective_nud | 1147-1173 | pub fn effective_nud(&self, interval: &Interval) -> u64 |
| allocator | RegisterAllocator::active_slot_count | 679-683 | pub fn active_slot_count(&self) -> usize |
| allocator | RegisterAllocator::alloc_single | 764-767 | pub fn alloc_single(&mut self, owner: SlotOwner) -> Result |
| allocator | RegisterAllocator::begin_scope | 713-715 | pub fn begin_scope(&mut self) |
| allocator | RegisterAllocator::current_depth | 664-666 | pub fn current_depth(&self) -> usize |
| allocator | RegisterAllocator::end_scope | 725-732 | pub fn end_scope(&mut self) |
| allocator | RegisterAllocator::free_count | 687-689 | pub fn free_count(&self) -> usize |
| allocator | RegisterAllocator::is_register_free | 693-695 | pub fn is_register_free(&self, reg: u16) -> bool |
| allocator | RegisterAllocator::new | 240-252 | pub fn new() -> Self |
| allocator | RegisterAllocator::peak_reg | 673-675 | pub fn peak_reg(&self) -> u16 |
| allocator | RegisterAllocator::release_slot | 546-575 | pub fn release_slot(&mut self, handle: SlotHandle) |
| allocator | RegisterAllocator::release_slots_by_depth | 600-615 | pub fn release_slots_by_depth(&mut self, target_depth: us... |
| allocator | RegisterAllocator::reserve_remote | 791-831 | pub fn reserve_remote(&mut self, count: u16, owner: SlotO... |
| allocator | RegisterAllocator::reserve_slot | 293-350 | pub fn reserve_slot(&mut self, count: u16, owner: SlotOwn... |
| allocator | RegisterAllocator::slot_count | 653-655 | pub fn slot_count(&self, handle: SlotHandle) -> u16 |
| allocator | RegisterAllocator::slot_owner | 658-660 | pub fn slot_owner(&self, handle: SlotHandle) -> SlotOwner |
| allocator | RegisterAllocator::slot_range | 640-643 | pub fn slot_range(&self, handle: SlotHandle) -> (u16, u16) |
| allocator | RegisterAllocator::slot_start | 648-650 | pub fn slot_start(&self, handle: SlotHandle) -> u16 |
| allocator | RegisterAllocator::with_depth | 255-259 | pub fn with_depth(initial_depth: usize) -> Self |
| allocator | build_intervals | 1809-1844 | pub fn build_intervals(def_ips: &[Option], use_ips: &[Opt... |
| allocator | enhance_intervals | 1874-1910 | pub fn enhance_intervals(intervals: &mut [Interval], conf... |
| arena | RegionAllocator::allocate | 188-239 | pub fn allocate(&mut self, frame_idx: usize, size: usize,... |
| arena | RegionAllocator::allocate_object | 260-286 | pub fn allocate_object(&mut self, frame_idx: usize, obj: ... |
| arena | RegionAllocator::as_mut_slice | 434-436 | pub fn as_mut_slice(&mut self, offset: usize, len: usize)... |
| arena | RegionAllocator::as_slice | 424-426 | pub fn as_slice(&self, offset: usize, len: usize) -> &[u8] |
| arena | RegionAllocator::begin_frame | 164-175 | pub fn begin_frame(&mut self) -> usize |
| arena | RegionAllocator::config | 466-468 | pub fn config(&self) -> &RegionConfig |
| arena | RegionAllocator::depth | 448-450 | pub fn depth(&self) -> usize |
| arena | RegionAllocator::end_frame | 340-366 | pub fn end_frame(&mut self, frame_idx: usize, has_escape:... |
| arena | RegionAllocator::frame_objects | 403-409 | pub fn frame_objects(&self, frame_idx: usize) -> Option |
| arena | RegionAllocator::frame_state | 475-477 | pub fn frame_state(&self, frame_idx: usize) -> Option |
| arena | RegionAllocator::get_arena_object | 301-303 | pub fn get_arena_object(&self, arena_obj_idx: u32) -> Option |
| arena | RegionAllocator::get_arena_object_mut | 307-309 | pub fn get_arena_object_mut(&mut self, arena_obj_idx: u32... |
| arena | RegionAllocator::global_usage | 441-443 | pub fn global_usage(&self) -> usize |
| arena | RegionAllocator::mark_escaped | 320-324 | pub fn mark_escaped(&mut self, frame_idx: usize) |
| arena | RegionAllocator::new | 139-147 | pub fn new(config: RegionConfig) -> Self |
| arena | RegionAllocator::objects_len | 414-416 | pub fn objects_len(&self) -> usize |
| arena | RegionAllocator::reset | 456-461 | pub fn reset(&mut self) |
| arena | RegionAllocator::take_arena_object | 383-390 | pub fn take_arena_object(&mut self, arena_obj_idx: u32) -... |
| arena | RegionAllocator::with_default | 152-154 | pub fn with_default() -> Self |
| array | register | 73-79 | pub fn register(reg: &mut BuiltinRegistry) |
| ast | Expr::span | 1095-1126 | pub fn span(&self) -> &Span |
| ast | Span::new | 170-172 | pub fn new(line: usize, column: usize) -> Self |
| attr | AttrStruct::field | 379-382 | pub fn field(self, name: &str) -> Self |
| attr | AttrStruct::new | 375-377 | pub fn new(name: &str) -> Self |
| attr | AttrStruct::optional_field | 384-387 | pub fn optional_field(self, name: &str) -> Self |
| attr | AttrStruct::parse | 389-451 | pub fn parse(&self, attrs: &[Attribute]) -> syn::Result |
| attr | AttrValues::get_bool | 474-484 | pub fn get_bool(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_char | 535-540 | pub fn get_char(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_f32 | 521-526 | pub fn get_f32(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_f64 | 528-533 | pub fn get_f64(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_i64 | 493-498 | pub fn get_i64(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_ident | 500-505 | pub fn get_ident(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_path | 507-512 | pub fn get_path(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_raw | 463-465 | pub fn get_raw(&self, name: &str) -> Option |
| attr | AttrValues::get_string | 467-472 | pub fn get_string(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_usize | 486-491 | pub fn get_usize(&self, name: &str) -> syn::Result |
| attr | AttrValues::get_vec | 514-519 | pub fn get_vec<T>(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_bool | 551-558 | pub fn require_bool(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_char | 614-621 | pub fn require_char(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_f32 | 596-603 | pub fn require_f32(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_f64 | 605-612 | pub fn require_f64(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_i64 | 569-576 | pub fn require_i64(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_ident | 578-585 | pub fn require_ident(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_path | 587-594 | pub fn require_path(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_string | 542-549 | pub fn require_string(&self, name: &str) -> syn::Result |
| attr | AttrValues::require_usize | 560-567 | pub fn require_usize(&self, name: &str) -> syn::Result |
| attr | expand_from_meta_derive | 656-695 | pub fn expand_from_meta_derive(input: &syn::DeriveInput) ... |
| attr | find_attr | 628-630 | pub fn find_attr<'a>(attrs: &[Attribute], name: &str) -> ... |
| baseline | BaselineManager::default_path | 199-201 | pub fn default_path(&self) -> &std::path::Path |
| baseline | BaselineManager::load | 212-252 | pub fn load(&self, path: Option) -> Result |
| baseline | BaselineManager::new | 181-185 | pub fn new() -> Self |
| baseline | BaselineManager::save | 263-296 | pub fn save(&self, data: &BaselineData, path: Option) -> ... |
| baseline | BaselineManager::with_default_path | 192-196 | pub fn with_default_path<P>(path: P) -> Self |
| baseline | collect_environment_info | 309-315 | pub fn collect_environment_info() -> EnvironmentInfo |
| builder | IrBuilder::build | 95-102 | pub fn build(program: &ast::Program) -> Result |
| builder | IrBuilder::build_expr | 247-342 | pub fn build_expr(&mut self, expr: &Expr) -> Result |
| builder | IrBuilder::into_module | 1085-1087 | pub fn into_module(self) -> IrModule |
| builder | IrBuilder::new | 73-85 | pub fn new() -> Self |
| builder | NuzoBuilder::build | 74-76 | pub fn build(self) -> NuzoResult |
| builder | NuzoBuilder::capture_output | 37-40 | pub fn capture_output(self) -> Self |
| builder | NuzoBuilder::diagnostic | 55-58 | pub fn diagnostic(self) -> Self |
| builder | NuzoBuilder::diagnostic_with | 61-65 | pub fn diagnostic_with(self, max_errors: usize) -> Self |
| builder | NuzoBuilder::gc_threshold | 31-34 | pub fn gc_threshold(self, bytes: usize) -> Self |
| builder | NuzoBuilder::plugin | 68-71 | pub fn plugin(self, plugin: _) -> Self |
| builder | NuzoBuilder::trace | 43-46 | pub fn trace(self) -> Self |
| builder | NuzoBuilder::trace_with | 49-52 | pub fn trace_with(self, config: TraceConfig) -> Self |
| builtins | BuiltinRegistry::call | 493-506 | pub fn call(&self, name: &str, args: &[Value]) -> Option |
| builtins | BuiltinRegistry::get | 450-455 | pub fn get(&self, name: &str) -> Option |
| builtins | BuiltinRegistry::get_arity | 518-523 | pub fn get_arity(&self, name: &str) -> Option |
| builtins | BuiltinRegistry::is_empty | 572-574 | pub fn is_empty(&self) -> bool |
| builtins | BuiltinRegistry::len | 562-564 | pub fn len(&self) -> usize |
| builtins | BuiltinRegistry::names | 553-555 | pub fn names(&self) -> Vec |
| builtins | BuiltinRegistry::new | 341-383 | pub fn new() -> Self |
| builtins | BuiltinRegistry::register | 413-417 | pub fn register(&mut self, name: &str, func: BuiltinFn, a... |
| builtins | builtin_called_signal | 147-149 | pub fn builtin_called_signal() -> &Signal |
| builtins | configure_output_capture | 194-198 | pub fn configure_output_capture(capture: Option) |
| bus | SignalBus::clear | 425-434 | pub fn clear(&self) |
| bus | SignalBus::find | 322-359 | pub fn find<Args>(&self, name: &str) -> Result |
| bus | SignalBus::global | 213-215 | pub fn global() -> &SignalBus |
| bus | SignalBus::list_signals | 387-393 | pub fn list_signals(&self) -> Vec |
| bus | SignalBus::new | 187-192 | pub fn new() -> Self |
| bus | SignalBus::register | 251-275 | pub fn register<Args>(&self, signal: &Signal) -> Result |
| bytecode_assert | extract_instructions | 59-80 | pub fn extract_instructions(chunk: &Chunk) -> Vec |
| bytecode_assert | match_instruction | 120-174 | pub fn match_instruction(actual: &(Opcode, Vec), expected... |
| cache | BytecodeCache::cache | 637-660 | pub fn cache(&mut self, hash: &SourceHash, chunk: Chunk) ... |
| cache | BytecodeCache::clear | 707-711 | pub fn clear(&mut self) |
| cache | BytecodeCache::contains | 714-716 | pub fn contains(&self, hash: &SourceHash) -> bool |
| cache | BytecodeCache::hit_rate | 729-735 | pub fn hit_rate(&self) -> f64 |
| cache | BytecodeCache::invalidate | 689-692 | pub fn invalidate(&mut self, hash: &SourceHash) -> Result |
| cache | BytecodeCache::invalidate_batch | 695-704 | pub fn invalidate_batch(&mut self, hashes: &[SourceHash])... |
| cache | BytecodeCache::is_empty | 724-726 | pub fn is_empty(&self) -> bool |
| cache | BytecodeCache::len | 719-721 | pub fn len(&self) -> usize |
| cache | BytecodeCache::lookup | 662-673 | pub fn lookup(&mut self, hash: &SourceHash) -> Option |
| cache | BytecodeCache::lookup_mut | 675-686 | pub fn lookup_mut(&mut self, hash: &SourceHash) -> Option |
| cache | BytecodeCache::max_capacity | 765-767 | pub fn max_capacity(&self) -> usize |
| cache | BytecodeCache::max_capacity_mut | 765-767 | pub fn max_capacity_mut(&mut self, capacity: usize) |
| cache | BytecodeCache::new | 622-624 | pub fn new() -> Self |
| cache | BytecodeCache::reset_stats | 782-792 | pub fn reset_stats(&mut self) |
| cache | BytecodeCache::stats | 738-747 | pub fn stats(&self) -> (usize, usize, usize, f64, usize, ... |
| cache | BytecodeCache::top_accessed | 750-762 | pub fn top_accessed(&self, n: usize) -> Vec |
| cache | BytecodeCache::with_capacity | 626-635 | pub fn with_capacity(max_entries: usize) -> Self |
| cache | BytecodeCache::with_max_capacity | 772-774 | pub fn with_max_capacity(&mut self, capacity: usize) |
| cache | CacheGlobalStats::estimated_memory_usage | 1163-1174 | pub fn estimated_memory_usage(&self) -> usize |
| cache | CacheGlobalStats::is_healthy | 1147-1160 | pub fn is_healthy(&self) -> bool |
| cache | CacheManager::bytecode | 873-875 | pub fn bytecode(&self) -> &BytecodeCache |
| cache | CacheManager::bytecode_cache_capacity | 1069-1071 | pub fn bytecode_cache_capacity(&self) -> usize |
| cache | CacheManager::bytecode_cache_capacity_mut | 1074-1076 | pub fn bytecode_cache_capacity_mut(&mut self, capacity: u... |
| cache | CacheManager::bytecode_mut | 878-880 | pub fn bytecode_mut(&mut self) -> &mut BytecodeCache |
| cache | CacheManager::clear_all | 920-924 | pub fn clear_all(&mut self) |
| cache | CacheManager::get_inline_cache | 887-891 | pub fn get_inline_cache(&mut self, property_name: &str) -... |
| cache | CacheManager::global_stats | 942-1002 | pub fn global_stats(&self) -> CacheGlobalStats |
| cache | CacheManager::inline_cache_count | 911-913 | pub fn inline_cache_count(&self) -> usize |
| cache | CacheManager::invalidate_all_inline_caches | 904-908 | pub fn invalidate_all_inline_caches(&mut self) |
| cache | CacheManager::invalidate_inline_cache | 894-901 | pub fn invalidate_inline_cache(&mut self, property_name: ... |
| cache | CacheManager::new | 837-843 | pub fn new() -> Self |
| cache | CacheManager::print_stats_report | 1005-1047 | pub fn print_stats_report(&self) -> String |
| cache | CacheManager::reset_all_stats | 927-935 | pub fn reset_all_stats(&mut self) |
| cache | CacheManager::set_bytecode_cache_capacity | 1079-1081 | pub fn set_bytecode_cache_capacity(&mut self, capacity: u... |
| cache | CacheManager::set_string_pool_capacity | 1064-1066 | pub fn set_string_pool_capacity(&mut self, capacity: usize) |
| cache | CacheManager::string_pool_capacity | 1054-1056 | pub fn string_pool_capacity(&self) -> usize |
| cache | CacheManager::string_pool_capacity_mut | 1059-1061 | pub fn string_pool_capacity_mut(&mut self, capacity: usize) |
| cache | CacheManager::strings | 859-861 | pub fn strings(&self) -> &StringConstantPool |
| cache | CacheManager::strings_mut | 864-866 | pub fn strings_mut(&mut self) -> &mut StringConstantPool |
| cache | CacheManager::with_config | 846-852 | pub fn with_config(string_pool_capacity: usize, bytecode_... |
| cache | ICState::entry_count | 334-341 | pub fn entry_count(&self) -> usize |
| cache | ICState::is_megamorphic | 330-332 | pub fn is_megamorphic(&self) -> bool |
| cache | ICState::is_monomorphic | 322-324 | pub fn is_monomorphic(&self) -> bool |
| cache | ICState::is_polymorphic | 326-328 | pub fn is_polymorphic(&self) -> bool |
| cache | ICState::is_uninitialized | 318-320 | pub fn is_uninitialized(&self) -> bool |
| cache | ICState::name | 309-316 | pub fn name(&self) -> &str |
| cache | InlineCache::hit_rate | 510-516 | pub fn hit_rate(&self) -> f64 |
| cache | InlineCache::invalidate | 528-531 | pub fn invalidate(&mut self) |
| cache | InlineCache::lookup_or_update | 389-409 | pub fn lookup_or_update<F>(&mut self, shape_id: ShapeId, ... |
| cache | InlineCache::new | 377-384 | pub fn new() -> Self |
| cache | InlineCache::reset | 497-500 | pub fn reset(&mut self) |
| cache | InlineCache::reset_stats | 533-537 | pub fn reset_stats(&mut self) |
| cache | InlineCache::state | 502-504 | pub fn state(&self) -> &ICState |
| cache | InlineCache::state_mut | 506-508 | pub fn state_mut(&mut self) -> &mut ICState |
| cache | InlineCache::stats | 518-526 | pub fn stats(&self) -> (&str, usize, usize, f64, usize) |
| cache | InlineCache::update | 435-491 | pub fn update(&mut self, shape_id: ShapeId, offset: Prope... |
| cache | SourceHash::compute | 577-588 | pub fn compute(source: &[u8]) -> Self |
| cache | SourceHash::compute_str | 590-592 | pub fn compute_str(source: &str) -> Self |
| cache | SourceHash::value | 594-596 | pub fn value(&self) -> u64 |
| cache | StringConstantPool::clear | 227-232 | pub fn clear(&mut self) |
| cache | StringConstantPool::contains | 191-193 | pub fn contains(&self, s: &str) -> Option |
| cache | StringConstantPool::hit_rate | 208-214 | pub fn hit_rate(&self) -> f64 |
| cache | StringConstantPool::intern | 167-187 | pub fn intern(&mut self, s: &str) -> Result |
| cache | StringConstantPool::is_empty | 203-205 | pub fn is_empty(&self) -> bool |
| cache | StringConstantPool::len | 197-199 | pub fn len(&self) -> usize |
| cache | StringConstantPool::max_capacity | 236-238 | pub fn max_capacity(&self) -> usize |
| cache | StringConstantPool::max_capacity_mut | 241-247 | pub fn max_capacity_mut(&mut self, capacity: usize) |
| cache | StringConstantPool::new | 149-151 | pub fn new() -> Self |
| cache | StringConstantPool::set_max_capacity | 255 | pub fn set_max_capacity(&mut self, capacity: usize) |
| cache | StringConstantPool::set_max_capacity_value | 255 | pub fn set_max_capacity_value(&mut self, capacity: usize) |
| cache | StringConstantPool::stats | 217-224 | pub fn stats(&self) -> (usize, usize, usize, f64) |
| cache | StringConstantPool::with_capacity | 153-165 | pub fn with_capacity(max_capacity: usize) -> Self |
| cache | StringConstantPool::with_max_capacity | 252 | pub fn with_max_capacity(&mut self, capacity: usize) |
| classifier | ErrorClassifier::classify | 79-84 | pub fn classify(error: &NuzoError) -> (ErrorSeverity, Err... |
| classifier | ErrorClassifier::fix_suggestion | 281-330 | pub fn fix_suggestion(error: &NuzoError) -> String |
| classifier | ErrorClassifier::generate_fix_suggestion | 359-364 | pub fn generate_fix_suggestion(error: &NuzoError) -> Vec |
| classifier | ErrorClassifier::generate_root_cause | 245-250 | pub fn generate_root_cause(error: &NuzoError) -> String |
| classifier | ErrorClassifier::root_cause | 127-227 | pub fn root_cause(error: &InternalError) -> String |
| codegen | CodeGenerator::chunk | 183-185 | pub fn chunk(&self) -> &Chunk |
| codegen | CodeGenerator::generate | 156-174 | pub fn generate(&mut self, module: &IrModule) -> Result |
| codegen | CodeGenerator::into_chunk | 177-180 | pub fn into_chunk(self) -> Chunk |
| codegen | CodeGenerator::new | 137-147 | pub fn new() -> Self |
| collector | ErrorCollector::calculate_similarity | 1213-1228 | pub fn calculate_similarity(&self, a: &DiagnosticError, b... |
| collector | ErrorCollector::clear | 682-687 | pub fn clear(&mut self) |
| collector | ErrorCollector::cluster_errors_simple | 1430-1493 | pub fn cluster_errors_simple(&self) -> Vec |
| collector | ErrorCollector::collect_error | 420-482 | pub fn collect_error(&mut self, error: NuzoError, context... |
| collector | ErrorCollector::collect_nuzo_error | 523-602 | pub fn collect_nuzo_error(&mut self, error: NuzoError, co... |
| collector | ErrorCollector::detect_repeating_patterns | 1359-1412 | pub fn detect_repeating_patterns(&self) -> Vec |
| collector | ErrorCollector::disable | 331-333 | pub fn disable(&mut self) |
| collector | ErrorCollector::enable | 325-328 | pub fn enable(&mut self) |
| collector | ErrorCollector::error_count | 667-669 | pub fn error_count(&self) -> usize |
| collector | ErrorCollector::errors | 662-664 | pub fn errors(&self) -> &[DiagnosticError] |
| collector | ErrorCollector::export_full_report | 1163-1206 | pub fn export_full_report(&self) -> String |
| collector | ErrorCollector::export_json | 1051-1059 | pub fn export_json(&self) -> String |
| collector | ErrorCollector::export_json_compact | 1074-1082 | pub fn export_json_compact(&self) -> String |
| collector | ErrorCollector::export_json_pretty | 1051-1059 | pub fn export_json_pretty(&self) -> String |
| collector | ErrorCollector::export_to_file | 1114-1120 | pub fn export_to_file(&self, path: &str) -> Result |
| collector | ErrorCollector::get_json_stats | 1135-1139 | pub fn get_json_stats(&self) -> JsonValue |
| collector | ErrorCollector::get_practical_fix_priority | 1507-1540 | pub fn get_practical_fix_priority(&self) -> Vec |
| collector | ErrorCollector::handle_error_in_diagnostic_mode | 612-641 | pub fn handle_error_in_diagnostic_mode<F>(&mut self, erro... |
| collector | ErrorCollector::has_errors | 672-674 | pub fn has_errors(&self) -> bool |
| collector | ErrorCollector::is_enabled | 336-338 | pub fn is_enabled(&self) -> bool |
| collector | ErrorCollector::max_errors | 341-343 | pub fn max_errors(&mut self, max: usize) |
| collector | ErrorCollector::new | 311-322 | pub fn new() -> Self |
| collector | ErrorCollector::print_compact_report | 980-1031 | pub fn print_compact_report(&self) |
| collector | ErrorCollector::print_full_report | 694-820 | pub fn print_full_report(&self) |
| collector | ErrorCollector::print_warning_summary | 371-415 | pub fn print_warning_summary(&self) |
| collector | ErrorCollector::record_instruction | 352-356 | pub fn record_instruction(&mut self) |
| collector | ErrorCollector::smart_deduplicate | 1312-1351 | pub fn smart_deduplicate(&self) -> DeduplicationReport |
| collector | ErrorCollector::statistics | 677-679 | pub fn statistics(&self) -> &ErrorStatistics |
| collector | ErrorCollector::stop_on_fatal | 346-348 | pub fn stop_on_fatal(&mut self, stop: bool) |
| collector | ErrorCollector::with_stop_on_fatal | 1678-1680 | pub fn with_stop_on_fatal(&mut self, stop: bool) |
| compile_runner | compile_and_run | 29-53 | pub fn compile_and_run(source: &str) -> Result |
| compile_runner | compile_and_run_value | 79-115 | pub fn compile_and_run_value(source: &str) -> Result |
| compiler | CompileError::column | 510-515 | pub fn column(&self) -> Option |
| compiler | CompileError::line | 484-507 | pub fn line(&self) -> usize |
| compiler | Compiler::builder | 1003-1010 | pub fn builder() -> CompilerBuilder |
| compiler | Compiler::compile | 1213-1300 | pub fn compile(source: &str) -> Result |
| compiler | Compiler::compile_direct | 1702-1713 | pub fn compile_direct(source: &str) -> Result |
| compiler | Compiler::compile_program | 1351-1393 | pub fn compile_program(&mut self, program: &ast::Program)... |
| compiler | Compiler::compile_stmt | 1485-1526 | pub fn compile_stmt(&mut self, stmt: &ast::Stmt) -> Result |
| compiler | Compiler::into_chunk | 1034-1040 | pub fn into_chunk(self) -> Chunk |
| compiler | Compiler::is_lsra_enabled | 2009-2011 | pub fn is_lsra_enabled(&self) -> bool |
| compiler | Compiler::is_register_free | 1046-1048 | pub fn is_register_free(&self, reg: u16) -> bool |
| compiler | Compiler::lsra_mapping | 1984-1986 | pub fn lsra_mapping(&self) -> Option |
| compiler | Compiler::lsra_phys_reg | 2001-2005 | pub fn lsra_phys_reg(&self, vreg: u16) -> Option |
| compiler | Compiler::try_lsra_allocate | 1815-1870 | pub fn try_lsra_allocate(&mut self) -> Result |
| compiler | CompilerBuilder::build | 696-700 | pub fn build(self) -> Compiler |
| compiler | CompilerBuilder::source | 629-632 | pub fn source(self, source: _) -> Self |
| compiler | CompilerBuilder::source_file | 638-641 | pub fn source_file(self, file: _) -> Self |
| compiler | CompilerBuilder::with_lsra | 667-670 | pub fn with_lsra(self, enabled: bool) -> Self |
| compiler | CompilerBuilder::with_nud_config | 686-689 | pub fn with_nud_config(self, config: NudConfig) -> Self |
| compiler | compile_finished_signal | 133-135 | pub fn compile_finished_signal() -> &Signal |
| compiler | compile_function_done_signal | 145-147 | pub fn compile_function_done_signal() -> &Signal |
| compiler | compile_scope_entered_signal | 137-139 | pub fn compile_scope_entered_signal() -> &Signal |
| compiler | compile_scope_exited_signal | 141-143 | pub fn compile_scope_exited_signal() -> &Signal |
| compiler | compile_started_signal | 129-131 | pub fn compile_started_signal() -> &Signal |
| config | RunConfig::from_args | 67-115 | pub fn from_args(args: &[String]) -> (Self, Vec, bool) |
| config | RunConfig::script_path | 118-120 | pub fn script_path<'a>(&self, positional: &[String]) -> O... |
| config | print_nuzo_e2e_usage | 171-182 | pub fn print_nuzo_e2e_usage() |
| config | print_nuzo_run_usage | 144-168 | pub fn print_nuzo_run_usage() |
| config | print_nuzo_usage | 124-141 | pub fn print_nuzo_usage() |
| context | RuntimeContext::alloc_box | 135-143 | pub fn alloc_box(&mut self, val: Value) -> usize |
| context | RuntimeContext::alloc_heap | 107-115 | pub fn alloc_heap(&mut self, obj: HeapObject) -> u64 |
| context | RuntimeContext::box_count | 167-169 | pub fn box_count(&self) -> usize |
| context | RuntimeContext::get_box | 148-151 | pub fn get_box(&self, idx: usize) -> Option |
| context | RuntimeContext::get_string | 92-95 | pub fn get_string(&self, idx: u32) -> Option |
| context | RuntimeContext::heap | 120-123 | pub fn heap(&self, idx: u64) -> Option |
| context | RuntimeContext::heap_count | 126-128 | pub fn heap_count(&self) -> usize |
| context | RuntimeContext::intern_string | 73-87 | pub fn intern_string(&mut self, s: &str) -> u32 |
| context | RuntimeContext::new | 56-63 | pub fn new() -> Self |
| context | RuntimeContext::set_box | 156-164 | pub fn set_box(&mut self, idx: usize, val: Value) -> Result |
| context | RuntimeContext::string_count | 98-100 | pub fn string_count(&self) -> usize |
| control_stack | ControlStack::depth | 281-283 | pub fn depth(&self) -> usize |
| control_stack | ControlStack::is_empty | 268-270 | pub fn is_empty(&self) -> bool |
| control_stack | ControlStack::last | 240-242 | pub fn last(&self) -> Option |
| control_stack | ControlStack::last_mut | 240-242 | pub fn last_mut(&mut self) -> Option |
| control_stack | ControlStack::new | 166-170 | pub fn new() -> Self |
| control_stack | ControlStack::pop_and_prepare_patches | 331-344 | pub fn pop_and_prepare_patches(&mut self, loop_end: usize... |
| control_stack | ControlStack::pop_context | 214-219 | pub fn pop_context(&mut self, line: usize) -> Result |
| control_stack | ControlStack::push_context | 192-194 | pub fn push_context(&mut self, start_ip: usize) |
| control_stack | LoopContext::new | 101-108 | pub fn new(start_ip: usize) -> Self |
| control_stack | PatchInfo::has_patches | 407-409 | pub fn has_patches(&self) -> bool |
| control_stack | PatchInfo::total_patches | 396-398 | pub fn total_patches(&self) -> usize |
| convert | register | 130-141 | pub fn register(reg: &mut BuiltinRegistry) |
| debug | register | 124-129 | pub fn register(reg: &mut BuiltinRegistry) |
| debug_reporter | DebugEvent::new | 205-213 | pub fn new(phase: DebugPhase, message: _) -> Self |
| debug_reporter | DebugEvent::with_detail | 216-228 | pub fn with_detail(phase: DebugPhase, message: _, detail:... |
| debug_reporter | DebugPhase::display_name | 50-59 | pub fn display_name(&self) -> &str |
| debug_reporter | DebugPhase::execution_phases | 74-83 | pub fn execution_phases() -> _ |
| debug_reporter | DebugPhase::label | 62-71 | pub fn label(&self) -> &str |
| debug_reporter | DebugReporter::current_phase | 322-324 | pub fn current_phase(&self) -> DebugPhase |
| debug_reporter | DebugReporter::enter_phase | 296-304 | pub fn enter_phase(&mut self, phase: DebugPhase) |
| debug_reporter | DebugReporter::event_count | 343-345 | pub fn event_count(&self) -> usize |
| debug_reporter | DebugReporter::events_for_phase | 348-353 | pub fn events_for_phase(&self, phase: DebugPhase) -> Vec |
| debug_reporter | DebugReporter::exit_phase | 307-319 | pub fn exit_phase(&mut self, phase: DebugPhase) |
| debug_reporter | DebugReporter::format_report | 381-430 | pub fn format_report(&self) -> String |
| debug_reporter | DebugReporter::format_summary | 433-464 | pub fn format_summary(&self) -> String |
| debug_reporter | DebugReporter::is_enabled | 291-293 | pub fn is_enabled(&self) -> bool |
| debug_reporter | DebugReporter::new | 267-275 | pub fn new(enabled: bool) -> Self |
| debug_reporter | DebugReporter::phase_duration | 356-362 | pub fn phase_duration(&self, phase: DebugPhase) -> Option |
| debug_reporter | DebugReporter::record | 327-332 | pub fn record(&mut self, detail: DebugDetail, msg: _) |
| debug_reporter | DebugReporter::record_message | 335-340 | pub fn record_message(&mut self, msg: _) |
| debug_reporter | DebugReporter::set_source | 278-287 | pub fn set_source(&mut self, path: _, source: &str) |
| diag | Diagnostic::emit | 83-91 | pub fn emit(&self) -> proc_macro2::TokenStream |
| diag | Diagnostic::help | 113-115 | pub fn help(&self) -> Option |
| diag | Diagnostic::level | 101-103 | pub fn level(&self) -> &DiagnosticLevel |
| diag | Diagnostic::message | 105-107 | pub fn message(&self) -> &str |
| diag | Diagnostic::new | 57-65 | pub fn new(level: DiagnosticLevel, message: _) -> Self |
| diag | Diagnostic::note | 117-119 | pub fn note(&self) -> Option |
| diag | Diagnostic::span | 109-111 | pub fn span(&self) -> Option |
| diag | Diagnostic::to_compile_error | 94-98 | pub fn to_compile_error(&self) -> proc_macro2::TokenStream |
| diag | Diagnostic::with_help | 72-75 | pub fn with_help(self, help: _) -> Self |
| diag | Diagnostic::with_note | 77-80 | pub fn with_note(self, note: _) -> Self |
| diag | Diagnostic::with_span | 67-70 | pub fn with_span(self, span: Span) -> Self |
| diag | MultiDiagnostic::add | 203-205 | pub fn add(&mut self, diag: Diagnostic) |
| diag | MultiDiagnostic::emit_all | 218-224 | pub fn emit_all(&self) -> proc_macro2::TokenStream |
| diag | MultiDiagnostic::has_errors | 211-215 | pub fn has_errors(&self) -> bool |
| diag | MultiDiagnostic::into_result | 227-233 | pub fn into_result(self) -> Result |
| diag | MultiDiagnostic::is_empty | 207-209 | pub fn is_empty(&self) -> bool |
| diag | MultiDiagnostic::iter | 239-241 | pub fn iter(&self) -> _ |
| diag | MultiDiagnostic::len | 235-237 | pub fn len(&self) -> usize |
| diag | MultiDiagnostic::new | 199-201 | pub fn new() -> Self |
| diag | SpannedError::into_inner | 165-167 | pub fn into_inner(self) -> syn::Error |
| diag | SpannedError::new | 147-152 | pub fn new(span: Span, message: _) -> Self |
| diag | SpannedError::new_spanned | 154-159 | pub fn new_spanned(spanned: _, message: _) -> Self |
| diag | SpannedError::to_compile_error | 161-163 | pub fn to_compile_error(&self) -> proc_macro2::TokenStream |
| diag | error | 249-251 | pub fn error(msg: _) -> Diagnostic |
| diag | error_at | 254-256 | pub fn error_at(span: Span, msg: _) -> Diagnostic |
| diag | warning | 259-261 | pub fn warning(msg: _) -> Diagnostic |
| diagnostic | DiagnosticError::as_nuzo_error | 174-176 | pub fn as_nuzo_error(&self) -> Option |
| diagnostic | DiagnosticError::diagnosis | 183-185 | pub fn diagnosis(&self) -> Option |
| diagnostic | DiagnosticError::from_nuzo_error | 114-149 | pub fn from_nuzo_error(id: usize, nuzo_error: NuzoError, ... |
| diagnostic | DiagnosticError::is_internal_error | 165-167 | pub fn is_internal_error(&self) -> bool |
| diagnostic | DiagnosticError::is_nuzo_error | 156-158 | pub fn is_nuzo_error(&self) -> bool |
| diagnostic | DiagnosticError::new | 62-84 | pub fn new(id: usize, error: NuzoError, context: Executio... |
| discover | expand_discover | 133-165 | pub fn expand_discover(path_lit: &LitStr) -> syn::Result |
| dispatch | VM::execute | 266-336 | pub fn execute(&mut self, opcode: Opcode) -> Result |
| dispatch_gen | expand_define_dispatch_auto | 53-100 | pub fn expand_define_dispatch_auto(input: proc_macro2::To... |
| dispatch_table | dispatch_opcode_fast | 968-981 | pub fn dispatch_opcode_fast(vm: &mut super::VM, opcode: O... |
| display | IrModule::validate | 180-185 | pub fn validate(&self) -> Result |
| e2e_runner | E2eRunner::discover_tests | 76-104 | pub fn discover_tests(&self) -> Vec |
| e2e_runner | E2eRunner::new | 63-67 | pub fn new(test_dir: _) -> Self |
| e2e_runner | E2eRunner::run_all | 196-221 | pub fn run_all(&self) -> Vec |
| e2e_runner | E2eRunner::run_single_test | 122-187 | pub fn run_single_test(&self, path: &PathBuf) -> TestResult |
| e2e_runner | format_results | 247-369 | pub fn format_results(results: &[TestResult]) -> String |
| elastic_register_file | ElasticRegisterFile::activate | 823 | pub fn activate(&self) |
| elastic_register_file | ElasticRegisterFile::as_mut_slice | 818 | pub fn as_mut_slice(&mut self) -> &mut [Value] |
| elastic_register_file | ElasticRegisterFile::as_slice | 815 | pub fn as_slice(&self) -> &[Value] |
| elastic_register_file | ElasticRegisterFile::capacity | 780 | pub fn capacity(&self) -> usize |
| elastic_register_file | ElasticRegisterFile::clear | 806 | pub fn clear(&mut self) |
| elastic_register_file | ElasticRegisterFile::copy_within | 810-812 | pub fn copy_within(&mut self, src: std::ops::Range, dest_... |
| elastic_register_file | ElasticRegisterFile::deactivate | 826 | pub fn deactivate(&self) |
| elastic_register_file | ElasticRegisterFile::first | 820 | pub fn first(&self) -> Option |
| elastic_register_file | ElasticRegisterFile::get | 783 | pub fn get(&self, index: usize) -> Option |
| elastic_register_file | ElasticRegisterFile::is_empty | 777 | pub fn is_empty(&self) -> bool |
| elastic_register_file | ElasticRegisterFile::len | 774 | pub fn len(&self) -> usize |
| elastic_register_file | ElasticRegisterFile::new | 767-771 | pub fn new() -> Self |
| elastic_register_file | ElasticRegisterFile::pop | 800 | pub fn pop(&mut self) -> Option |
| elastic_register_file | ElasticRegisterFile::push | 798 | pub fn push(&mut self, value: Value) |
| elastic_register_file | ElasticRegisterFile::resize | 804 | pub fn resize(&mut self, new_len: usize, value: Value) |
| elastic_register_file | ElasticRegisterFile::set | 790 | pub fn set(&mut self, index: usize, value: Value) |
| elastic_register_file | ElasticRegisterFile::truncate | 802 | pub fn truncate(&mut self, new_len: usize) |
| elastic_register_file | ElasticRegisterFileInner::activate | 558-560 | pub fn activate(&self) |
| elastic_register_file | ElasticRegisterFileInner::as_mut_slice | 463-466 | pub fn as_mut_slice(&mut self) -> &mut [Value] |
| elastic_register_file | ElasticRegisterFileInner::as_slice | 456-459 | pub fn as_slice(&self) -> &[Value] |
| elastic_register_file | ElasticRegisterFileInner::capacity | 305-307 | pub fn capacity(&self) -> usize |
| elastic_register_file | ElasticRegisterFileInner::clear | 415-423 | pub fn clear(&mut self) |
| elastic_register_file | ElasticRegisterFileInner::copy_within | 426-452 | pub fn copy_within(&mut self, src_start: usize, src_end: ... |
| elastic_register_file | ElasticRegisterFileInner::deactivate | 563-569 | pub fn deactivate(&self) |
| elastic_register_file | ElasticRegisterFileInner::first | 469-475 | pub fn first(&self) -> Option |
| elastic_register_file | ElasticRegisterFileInner::get | 311-318 | pub fn get(&self, index: usize) -> Option |
| elastic_register_file | ElasticRegisterFileInner::is_empty | 300-302 | pub fn is_empty(&self) -> bool |
| elastic_register_file | ElasticRegisterFileInner::len | 295-297 | pub fn len(&self) -> usize |
| elastic_register_file | ElasticRegisterFileInner::new | 191-277 | pub fn new(reserve_slots: usize, initial_slots: usize) ->... |
| elastic_register_file | ElasticRegisterFileInner::pop | 368-375 | pub fn pop(&mut self) -> Option |
| elastic_register_file | ElasticRegisterFileInner::push | 358-365 | pub fn push(&mut self, value: Value) |
| elastic_register_file | ElasticRegisterFileInner::resize | 396-412 | pub fn resize(&mut self, new_len: usize, value: Value) |
| elastic_register_file | ElasticRegisterFileInner::set | 289-292 | pub fn set(&mut self, index: usize, value: Value) |
| elastic_register_file | ElasticRegisterFileInner::truncate | 378-393 | pub fn truncate(&mut self, new_len: usize) |
| encoding | Encoding::detect | 297-317 | pub fn detect(bytes: &[u8]) -> Encoding |
| encoding | Encoding::from_name | 286-295 | pub fn from_name(name: &str) -> Option |
| encoding | Encoding::name | 276-284 | pub fn name(self) -> &str |
| encoding | StringIndexCache::char_at_fast | 216-231 | pub fn char_at_fast(&mut self, s: &str, index: usize) -> ... |
| encoding | StringIndexCache::char_len_cached | 211-214 | pub fn char_len_cached(&self) -> usize |
| encoding | StringIndexCache::char_slice_fast | 233-249 | pub fn char_slice_fast<'a>(&mut self, s: &str, start: usi... |
| encoding | StringIndexCache::char_to_byte_index_fast | 251-257 | pub fn char_to_byte_index_fast(&mut self, s: &str, char_i... |
| encoding | StringIndexCache::ensure_built | 199-205 | pub fn ensure_built(&mut self, s: &str) |
| encoding | StringIndexCache::is_built | 207-209 | pub fn is_built(&self) -> bool |
| encoding | StringIndexCache::new | 195-197 | pub fn new() -> Self |
| encoding | char_at | 216-231 | pub fn char_at(s: &str, index: usize) -> Option |
| encoding | char_len | 211-214 | pub fn char_len(s: &str) -> usize |
| encoding | char_slice | 233-249 | pub fn char_slice(s: &str, start: usize, end: usize) -> Cow |
| encoding | char_to_byte_index | 251-257 | pub fn char_to_byte_index(s: &str, char_index: usize) -> ... |
| encoding | char_to_byte_offset | 420-425 | pub fn char_to_byte_offset(s: &str, char_pos: usize) -> u... |
| encoding | decode_from_bytes | 495-517 | pub fn decode_from_bytes(bytes: &[u8], encoding: Encoding... |
| encoding | encode_to_bytes | 463-493 | pub fn encode_to_bytes(s: &str, encoding: Encoding) -> Re... |
| encoding | utf8_truncate | 452-461 | pub fn utf8_truncate(s: &str, max_chars: usize) -> &str |
| error_kind | expand_error_kind | 269-324 | pub fn expand_error_kind(input: &DeriveInput) -> syn::Result |
| error_replay | ErrorReplay::extract_error_patterns | 261-324 | pub fn extract_error_patterns(&self) -> Vec |
| error_replay | ErrorReplay::from_error | 145-172 | pub fn from_error(error: &NuzoError, original_source: &st... |
| error_replay | ErrorReplay::generate_full_test_function | 226-245 | pub fn generate_full_test_function(&self) -> String |
| error_replay | ErrorReplay::generate_nuzo_test_macro | 185-216 | pub fn generate_nuzo_test_macro(&self) -> String |
| error_replay | generate_replay_test | 553-559 | pub fn generate_replay_test(source: &str) -> Option |
| errors | NuzoError::arithmetic_overflow | 425-430 | pub fn arithmetic_overflow() -> Self |
| errors | NuzoError::assert_failed | 433-440 | pub fn assert_failed(message: _) -> Self |
| errors | NuzoError::division_by_zero | 417-422 | pub fn division_by_zero() -> Self |
| errors | NuzoError::expected_number | 443-448 | pub fn expected_number(got: _) -> Self |
| errors | NuzoError::index_out_of_bounds | 406-414 | pub fn index_out_of_bounds(index: _, length: _) -> Self |
| errors | NuzoError::internal | 478-483 | pub fn internal(err: InternalError, diagnosis: Option) ->... |
| errors | NuzoError::invalid_argument_count | 451-456 | pub fn invalid_argument_count(expected: usize, got: usize... |
| errors | NuzoError::type_mismatch | 395-403 | pub fn type_mismatch(expected: _, actual: _) -> Self |
| errors | NuzoError::undefined_variable | 459-464 | pub fn undefined_variable(name: _) -> Self |
| errors | NuzoError::unsupported_operation | 467-475 | pub fn unsupported_operation(operation: _, type_name: _) ... |
| errors | NuzoError::with_source_location | 499-502 | pub fn with_source_location(self, loc: SourceLocation) ->... |
| expressions | Compiler::compile_expr | 375-492 | pub fn compile_expr(&mut self, expr: &ast::Expr) -> Result |
| formatter | AnsiStyle::apply_to | 131-155 | pub fn apply_to(&self, text: _) -> StyledText |
| formatter | DiagnosticFormatter::bottom_border | 372-376 | pub fn bottom_border(&self) -> String |
| formatter | DiagnosticFormatter::cyan_style | 339-345 | pub fn cyan_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::dim_style | 321-327 | pub fn dim_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::error_style | 286-292 | pub fn error_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::fatal_style | 275-281 | pub fn fatal_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::info_style | 308-314 | pub fn info_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::new | 230-234 | pub fn new() -> Self |
| formatter | DiagnosticFormatter::no_color | 239-244 | pub fn no_color() -> Self |
| formatter | DiagnosticFormatter::no_color_with_width | 247-252 | pub fn no_color_with_width(width: usize) -> Self |
| formatter | DiagnosticFormatter::section_header | 385-394 | pub fn section_header(&self, emoji: &str, title: &str) ->... |
| formatter | DiagnosticFormatter::separator | 354-358 | pub fn separator(&self) -> String |
| formatter | DiagnosticFormatter::severity_emoji | 411-418 | pub fn severity_emoji(&self, severity: ErrorSeverity) -> ... |
| formatter | DiagnosticFormatter::severity_label | 421-428 | pub fn severity_label(&self, severity: ErrorSeverity) -> ... |
| formatter | DiagnosticFormatter::severity_style | 401-408 | pub fn severity_style(&self, severity: ErrorSeverity) -> ... |
| formatter | DiagnosticFormatter::should_colorize | 264-266 | pub fn should_colorize(&self) -> bool |
| formatter | DiagnosticFormatter::success_style | 330-336 | pub fn success_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::top_border | 363-367 | pub fn top_border(&self) -> String |
| formatter | DiagnosticFormatter::warning_style | 297-303 | pub fn warning_style(&self) -> AnsiStyle |
| formatter | DiagnosticFormatter::width | 259-261 | pub fn width(&self) -> usize |
| formatter | StyledText::raw | 175-177 | pub fn raw(&self) -> &str |
| formatter | StyledText::styled | 180-182 | pub fn styled(&self) -> &str |
| function | DebugInfo::inlined_function_at | 181-185 | pub fn inlined_function_at(&self, ip: usize) -> Option |
| function | DebugInfo::is_dead_code_line | 191-195 | pub fn is_dead_code_line(&self, line: usize) -> Option |
| function | FunctionPrototype::new | 253-274 | pub fn new(name: String, arity: u8, locals_count: u16, ch... |
| functions | collect_all_identifiers | 573-581 | pub fn collect_all_identifiers(block: &ast::Block) -> Vec |
| functions | collect_assigned_vars | 710-718 | pub fn collect_assigned_vars(block: &ast::Block) -> std::... |
| gc | Gc::alloc | 565-568 | pub fn alloc(&mut self, obj: HeapObject) -> u32 |
| gc | Gc::alloc_bulk | 581-596 | pub fn alloc_bulk(&mut self, objects: Vec) -> Vec |
| gc | Gc::alloc_scratch | 785-792 | pub fn alloc_scratch(&mut self, obj: HeapObject) -> u32 |
| gc | Gc::alloc_scratch_with_size | 795 | pub fn alloc_scratch_with_size(&mut self, obj: HeapObject... |
| gc | Gc::alloc_uninit | 598-606 | pub fn alloc_uninit(&mut self, size: usize) -> u32 |
| gc | Gc::alloc_with_size | 571-579 | pub fn alloc_with_size(&mut self, obj: HeapObject, size: ... |
| gc | Gc::chunk_info | 1251-1264 | pub fn chunk_info(&self) -> Vec |
| gc | Gc::clear | 1234-1249 | pub fn clear(&mut self) |
| gc | Gc::collect | 1029-1192 | pub fn collect(&mut self) |
| gc | Gc::collect_with_roots | 1194-1199 | pub fn collect_with_roots(&mut self, roots: _) |
| gc | Gc::commit | 608-622 | pub fn commit(&mut self, idx: u32, obj: HeapObject) |
| gc | Gc::get | 857-868 | pub fn get(&self, idx: u32) -> &HeapObject |
| gc | Gc::get_mut | 871-889 | pub fn get_mut(&mut self, idx: u32) -> &mut HeapObject |
| gc | Gc::get_mut_if_present | 892-908 | pub fn get_mut_if_present(&mut self, idx: u32) -> Option |
| gc | Gc::is_empty | 1232 | pub fn is_empty(&self) -> bool |
| gc | Gc::len | 1231 | pub fn len(&self) -> usize |
| gc | Gc::mark_index | 471-482 | pub fn mark_index(&mut self, idx: u32) |
| gc | Gc::mark_roots | 924-933 | pub fn mark_roots(&mut self, roots: _) |
| gc | Gc::new | 429-462 | pub fn new(threshold: usize) -> Self |
| gc | Gc::promote_from_region | 818-820 | pub fn promote_from_region(&mut self, obj: HeapObject, si... |
| gc | Gc::register_roots_fn | 1229 | pub fn register_roots_fn(&mut self, f: Option) |
| gc | Gc::safe_point | 828-849 | pub fn safe_point<F>(&mut self, scan_roots: F) -> Vec |
| gc | Gc::scratch_data_ptr | 851 | pub fn scratch_data_ptr(&self) -> *const Option |
| gc | Gc::scratch_stats | 852 | pub fn scratch_stats(&self) -> (u64, u64, u64) |
| gc | Gc::set_gc_threshold | 1221-1225 | pub fn set_gc_threshold(&mut self, t: usize) |
| gc | Gc::stats | 1213-1217 | pub fn stats(&self) -> GcStats |
| gc | Gc::sweep | 936-1015 | pub fn sweep(&mut self) |
| gc | Gc::threshold | 1219 | pub fn threshold(&self) -> usize |
| gc | Gc::try_get | 911-922 | pub fn try_get(&self, idx: u32) -> Option |
| gc | Gc::with_default_threshold | 464 | pub fn with_default_threshold() -> Self |
| gc | Gc::with_mark_rate | 467 | pub fn with_mark_rate(self, rate: usize) -> Self |
| gc | Gc::with_sweep_rate | 468 | pub fn with_sweep_rate(self, rate: usize) -> Self |
| gc | Gc::with_threshold | 465 | pub fn with_threshold(threshold: usize) -> Self |
| gc | Gc::with_threshold_value | 1227 | pub fn with_threshold_value(&mut self, t: usize) |
| gc | gc_did_collect_signal | 213 | pub fn gc_did_collect_signal() -> &Signal |
| gc | gc_will_collect_signal | 212 | pub fn gc_will_collect_signal() -> &Signal |
| gc | is_scratch | 72 | pub fn is_scratch(idx: u32) -> bool |
| gc | update_gc_chunks_ptr | 205-208 | pub fn update_gc_chunks_ptr(gc: &Gc) |
| gc_bench | bench_g10_freelist_reuse | 561-639 | pub fn bench_g10_freelist_reuse(config: &BenchmarkConfig)... |
| gc_bench | bench_g1_alloc_throughput | 170-187 | pub fn bench_g1_alloc_throughput(config: &BenchmarkConfig... |
| gc_bench | bench_g2_alloc_with_gc | 196-218 | pub fn bench_g2_alloc_with_gc(config: &BenchmarkConfig) -... |
| gc_bench | bench_g3_deep_graph_trace | 228-256 | pub fn bench_g3_deep_graph_trace(config: &BenchmarkConfig... |
| gc_bench | bench_g4_circular_ref | 266-291 | pub fn bench_g4_circular_ref(config: &BenchmarkConfig) ->... |
| gc_bench | bench_g5_mark_only | 301-323 | pub fn bench_g5_mark_only(config: &BenchmarkConfig) -> Be... |
| gc_bench | bench_g6_large_array_alloc | 336-415 | pub fn bench_g6_large_array_alloc(config: &BenchmarkConfi... |
| gc_bench | bench_g7_dict_alloc | 424-441 | pub fn bench_g7_dict_alloc(config: &BenchmarkConfig) -> B... |
| gc_bench | bench_g8_incremental_pacing | 451-473 | pub fn bench_g8_incremental_pacing(config: &BenchmarkConf... |
| gc_bench | bench_g9_chunk_batch_recovery | 486-547 | pub fn bench_g9_chunk_batch_recovery(config: &BenchmarkCo... |
| gc_bench | run_all_gc_benchmarks | 646-659 | pub fn run_all_gc_benchmarks(config: &BenchmarkConfig) ->... |
| generators | gen_smi_safe | 329-331 | pub fn gen_smi_safe(g: &mut Gen) -> nuzo_values::Value |
| hardcode::env | EnvOverride::env_key | 81-83 | pub fn env_key(name: &str) -> String |
| hardcode::env | EnvOverride::get | 49-55 | pub fn get(name: &str) -> Option |
| hardcode::env | EnvOverride::get_as_bool | 71-78 | pub fn get_as_bool(name: &str) -> Option |
| hardcode::env | EnvOverride::get_as_f64 | 63-65 | pub fn get_as_f64(name: &str) -> Option |
| hardcode::env | EnvOverride::get_as_i64 | 58-60 | pub fn get_as_i64(name: &str) -> Option |
| hardcode::env | EnvOverride::list_overrides | 88-98 | pub fn list_overrides() -> Vec |
| hardcode::env | OverrideReader::as_bool | 141-143 | pub fn as_bool(&self) -> Option |
| hardcode::env | OverrideReader::as_f64 | 136-138 | pub fn as_f64(&self) -> Option |
| hardcode::env | OverrideReader::as_i64 | 131-133 | pub fn as_i64(&self) -> Option |
| hardcode::env | OverrideReader::as_str | 146-148 | pub fn as_str(&self) -> Option |
| hardcode::env | OverrideReader::new | 119-123 | pub fn new(name: &str) -> Self |
| hardcode::env | OverrideReader::or_string | 156-158 | pub fn or_string(self, default: &str) -> String |
| hardcode::env | OverrideReader::raw | 126-128 | pub fn raw(&self) -> Option |
| hardcode::export | ConstantExport::count | 55-57 | pub fn count(&self) -> usize |
| hardcode::export | ConstantExport::new | 48-52 | pub fn new() -> Self |
| hardcode::export | ConstantExport::to_json | 60-64 | pub fn to_json(&self) -> String |
| hardcode::export | ConstantExport::write_json | 69-91 | pub fn write_json<W>(&self, writer: W) -> std::io::Result |
| hardcode::registry | all | 93-107 | pub fn all() -> Vec |
| hardcode::registry | by_module | 124-141 | pub fn by_module(module_path: &str) -> Vec |
| hardcode::registry | by_type | 146-163 | pub fn by_type(type_name: &str) -> Vec |
| hardcode::registry | clear | 172-176 | pub fn clear() |
| hardcode::registry | count | 110-113 | pub fn count() -> usize |
| hardcode::registry | exists | 116-119 | pub fn exists(name: &str) -> bool |
| hardcode::registry | get | 79-90 | pub fn get(name: &str) -> Option |
| hardcode::registry | init | 72-76 | pub fn init(groups: &[RegisterFn]) |
| hardcode::registry | register | 52-60 | pub fn register(info: ConstantInfo) |
| hardcode::registry | register_group | 65-67 | pub fn register_group(register_fn: RegisterFn) |
| hardcode::types | ConstantInfo::doc_text | 81-87 | pub fn doc_text(&self) -> Option |
| hardcode::types | ConstantInfo::is_bool | 50-52 | pub fn is_bool(&self) -> bool |
| hardcode::types | ConstantInfo::is_float | 45-47 | pub fn is_float(&self) -> bool |
| hardcode::types | ConstantInfo::is_integer | 36-42 | pub fn is_integer(&self) -> bool |
| hardcode::types | ConstantInfo::is_string | 55-57 | pub fn is_string(&self) -> bool |
| hardcode::types | ConstantInfo::parse_as_f64 | 72-78 | pub fn parse_as_f64(&self) -> Option |
| hardcode::types | ConstantInfo::parse_as_i64 | 62-67 | pub fn parse_as_i64(&self) -> Option |
| hardcode::validate | ValidationResult::error_count | 61-63 | pub fn error_count(&self) -> usize |
| hardcode::validate | ValidationResult::fail | 52-58 | pub fn fail(name: &str, errors: Vec) -> Self |
| hardcode::validate | ValidationResult::ok | 43-49 | pub fn ok(name: &str) -> Self |
| hardcode::validate | clear_rules | 223-226 | pub fn clear_rules() |
| hardcode::validate | register_rule | 105-112 | pub fn register_rule(name: &str, rule: Rule) |
| hardcode::validate | rule_count | 229-232 | pub fn rule_count() -> usize |
| hardcode::validate | validate | 117-169 | pub fn validate(name: &str) -> Option |
| hardcode::validate | validate_all | 117-169 | pub fn validate_all() -> Vec |
| harness | BenchmarkConfig::new | 202-204 | pub fn new() -> Self |
| harness | BenchmarkConfig::with_params | 208-213 | pub fn with_params(warmup_iterations: usize, sample_size:... |
| harness | BenchmarkResult::deviation_from_target | 152-158 | pub fn deviation_from_target(&self) -> Option |
| harness | BenchmarkResult::meets_target | 137-147 | pub fn meets_target(&self) -> bool |
| harness | run_benchmark | 238-289 | pub fn run_benchmark<F>(id: &str, name: &str, unit: &str,... |
| hash | xx_hash_map | 72-74 | pub fn xx_hash_map<K, V>(capacity: usize) -> XxHashMap |
| hash | xx_hash_map_new | 90-92 | pub fn xx_hash_map_new<K, V>() -> XxHashMap |
| hash | xx_hash_set | 107-109 | pub fn xx_hash_set<K>(capacity: usize) -> XxHashSet |
| hash | xx_hash_set_new | 115-117 | pub fn xx_hash_set_new<K>() -> XxHashSet |
| heap | HeapObject::get_index | 350-363 | pub fn get_index(&self, idx: Value) -> Result |
| heap | HeapObject::get_prop | 309-317 | pub fn get_prop(&self, name: &str) -> Option |
| heap | HeapObject::obj_len | 338-344 | pub fn obj_len(&self) -> usize |
| heap | HeapObject::remap_scratch_indices | 252-303 | pub fn remap_scratch_indices(&mut self, remap: &[(u32, u3... |
| heap | HeapObject::set_index | 373-397 | pub fn set_index(&self, idx: Value, val: Value) -> Result |
| heap | HeapObject::set_index_mut | 417-441 | pub fn set_index_mut(&mut self, idx: Value, val: Value) -... |
| heap | HeapObject::set_prop | 321-332 | pub fn set_prop(&self, _name: &str, key_index: Option, va... |
| heap | HeapObject::set_prop_mut | 454-462 | pub fn set_prop_mut(&mut self, key_index: usize, val: Val... |
| heap | HeapObject::size_estimate | 215-234 | pub fn size_estimate(&self) -> usize |
| heap | HeapObject::type_name | 238-248 | pub fn type_name(&self) -> &str |
| hints | likely | 13-15 | pub fn likely(b: bool) -> bool |
| hints | unlikely | 22-24 | pub fn unlikely(b: bool) -> bool |
| inspector | ValueInspector::format_diagnostic | 254-257 | pub fn format_diagnostic(value: Value) -> String |
| inspector | ValueInspector::inspect | 198-223 | pub fn inspect(value: Value) -> InspectionReport |
| inspector | annotate_source_lines | 486-497 | pub fn annotate_source_lines(instructions: &mut [Disassem... |
| inspector | disassemble_chunk | 152-212 | pub fn disassemble_chunk(chunk: &Chunk) -> Vec |
| inspector | format_report | 504-510 | pub fn format_report(report: &InspectionReport, config: &... |
| inspector | infer_register_uses | 334-444 | pub fn infer_register_uses(instructions: &[DisassembledIn... |
| inspector | inspect | 1087-1142 | pub fn inspect(chunk: &Chunk, debug_info: &DebugInfo, _so... |
| io | register | 84-89 | pub fn register(reg: &mut BuiltinRegistry) |
| layout | ValueLayout::format_visual | 273-370 | pub fn format_visual(&self) -> String |
| layout | ValueLayout::from_value | 148-246 | pub fn from_value(value: Value) -> Self |
| layout | ValueLayout::validate | 385-449 | pub fn validate(&self) -> Result |
| lexer | Lexer::new | 290-306 | pub fn new(source: &str) -> Self |
| lexer | Lexer::scan_all | 333-347 | pub fn scan_all(self) -> Result |
| lib | bind | 32-38 | pub fn bind(input: TokenStream) -> TokenStream |
| lib | class | 28-54 | pub fn class(args: TokenStream, input: TokenStream) -> To... |
| lib | class_impl | 97-142 | pub fn class_impl(args: TokenStream, input: TokenStream) ... |
| lib | compile | 148-150 | pub fn compile(source: &str) -> NuzoResult |
| lib | constructor | 210-216 | pub fn constructor(_args: TokenStream, input: TokenStream... |
| lib | define_dispatch_auto | 267-271 | pub fn define_dispatch_auto(input: TokenStream) -> TokenS... |
| lib | define_opcodes | 236-240 | pub fn define_opcodes(input: TokenStream) -> TokenStream |
| lib | derive_from_meta | 56-61 | pub fn derive_from_meta(input: TokenStream) -> TokenStream |
| lib | derive_match_sync | 22-27 | pub fn derive_match_sync(input: TokenStream) -> TokenStream |
| lib | derive_opcode_sync | 142-147 | pub fn derive_opcode_sync(input: TokenStream) -> TokenStream |
| lib | derive_trace | 99-104 | pub fn derive_trace(input: TokenStream) -> TokenStream |
| lib | discover | 90-98 | pub fn discover(input: TokenStream) -> TokenStream |
| lib | eval | 138-140 | pub fn eval(source: &str) -> NuzoResult |
| lib | get | 222-228 | pub fn get(_args: TokenStream, input: TokenStream) -> Tok... |
| lib | method | 246-252 | pub fn method(_args: TokenStream, input: TokenStream) -> ... |
| lib | nuzo_test | 175-197 | pub fn nuzo_test(attr: TokenStream, item: TokenStream) ->... |
| lib | py_test | 113-120 | pub fn py_test(input: TokenStream) -> TokenStream |
| lib | run | 133-135 | pub fn run(source: &str) -> NuzoResult |
| lib | run_file | 143-145 | pub fn run_file(path: _) -> NuzoResult |
| lib | set | 234-240 | pub fn set(_args: TokenStream, input: TokenStream) -> Tok... |
| lib | signal_error_to_nuzo_error | 58-65 | pub fn signal_error_to_nuzo_error(e: nuzo_signal::SignalE... |
| lib | static_method | 258-264 | pub fn static_method(_args: TokenStream, input: TokenStre... |
| log | SignalLog::clear_filter | 397-401 | pub fn clear_filter(&self) |
| log | SignalLog::disable | 342-344 | pub fn disable(&self) |
| log | SignalLog::enable | 322-324 | pub fn enable(&self) |
| log | SignalLog::global | 309-311 | pub fn global() -> &SignalLog |
| log | SignalLog::is_enabled | 354-356 | pub fn is_enabled(&self) -> bool |
| log | SignalLog::log | 467-486 | pub fn log(&self, entry: LogEntry) |
| log | SignalLog::with_filter | 387-391 | pub fn with_filter<F>(&self, filter: F) |
| log | SignalLog::with_writer | 432-436 | pub fn with_writer<W>(&self, writer: W) |
| match_sync | expand_match_sync | 151-201 | pub fn expand_match_sync(input: &DeriveInput) -> syn::Result |
| math | register | 111-127 | pub fn register(reg: &mut BuiltinRegistry) |
| nuzo | Nuzo::builder | 30-32 | pub fn builder() -> NuzoBuilder |
| nuzo | Nuzo::compile | 108-110 | pub fn compile(&self, source: &str) -> NuzoResult |
| nuzo | Nuzo::enable_diagnostic_mode | 146-148 | pub fn enable_diagnostic_mode(&mut self) |
| nuzo | Nuzo::eval | 79-105 | pub fn eval(&mut self, source: &str) -> NuzoResult |
| nuzo | Nuzo::execute | 113-115 | pub fn execute(&mut self, chunk: Chunk) -> NuzoResult |
| nuzo | Nuzo::flush_output | 180-188 | pub fn flush_output(&self) |
| nuzo | Nuzo::global | 118-120 | pub fn global(&self, name: &str) -> Option |
| nuzo | Nuzo::last_call_stack | 161-163 | pub fn last_call_stack(&self) -> &[nuzo_error::StackFrame... |
| nuzo | Nuzo::print_diagnostic_report | 156-158 | pub fn print_diagnostic_report(&self) |
| nuzo | Nuzo::quick | 25-27 | pub fn quick() -> NuzoResult |
| nuzo | Nuzo::register_builtin | 128-136 | pub fn register_builtin(&mut self, name: &str, func: nuzo... |
| nuzo | Nuzo::reset | 139-141 | pub fn reset(&mut self) |
| nuzo | Nuzo::run | 73-76 | pub fn run(&mut self, source: &str) -> NuzoResult |
| nuzo | Nuzo::set_global | 123-125 | pub fn set_global(&mut self, name: &str, value: Value) |
| nuzo | Nuzo::take_tracer_result | 151-153 | pub fn take_tracer_result(&mut self) -> Option |
| nuzo | Nuzo::vm | 166-168 | pub fn vm(&self) -> &VM |
| nuzo | Nuzo::vm_mut | 171-173 | pub fn vm_mut(&mut self) -> &mut VM |
| nuzo | generate_nuzo_imports | 332-350 | pub fn generate_nuzo_imports(modules: &[NuzoModule]) -> p... |
| nuzo | make_path | 308-316 | pub fn make_path(segments: &[&str]) -> syn::Path |
| nuzo | nuzo_bytecode_path | 257-259 | pub fn nuzo_bytecode_path() -> syn::Path |
| nuzo | nuzo_compiler_path | 267-269 | pub fn nuzo_compiler_path() -> syn::Path |
| nuzo | nuzo_core_path | 287-289 | pub fn nuzo_core_path() -> syn::Path |
| nuzo | nuzo_crate_path | 238-247 | pub fn nuzo_crate_path() -> syn::Path |
| nuzo | nuzo_error_path | 277-279 | pub fn nuzo_error_path() -> syn::Path |
| nuzo | nuzo_frontend_path | 272-274 | pub fn nuzo_frontend_path() -> syn::Path |
| nuzo | nuzo_helpers_path | 282-284 | pub fn nuzo_helpers_path() -> syn::Path |
| nuzo | nuzo_value_path | 252-254 | pub fn nuzo_value_path() -> syn::Path |
| nuzo | nuzo_vm_path | 262-264 | pub fn nuzo_vm_path() -> syn::Path |
| nuzo_dict | LargeDict::contains_value | 455-457 | pub fn contains_value(&self, target: &Value) -> bool |
| nuzo_dict | LargeDict::from_entries | 248-266 | pub fn from_entries(entries: Vec) -> Self |
| nuzo_dict | LargeDict::get | 334-338 | pub fn get(&self, key_index: u32) -> Option |
| nuzo_dict | LargeDict::insert | 269-308 | pub fn insert(&mut self, key_index: u32, value: Value) |
| nuzo_dict | LargeDict::is_empty | 427-429 | pub fn is_empty(&self) -> bool |
| nuzo_dict | LargeDict::iter | 431-437 | pub fn iter(&self) -> _ |
| nuzo_dict | LargeDict::len | 422-424 | pub fn len(&self) -> usize |
| nuzo_dict | LargeDict::new | 227-229 | pub fn new() -> Self |
| nuzo_dict | LargeDict::values | 439-445 | pub fn values(&self) -> _ |
| nuzo_dict | LargeDict::values_mut | 447-453 | pub fn values_mut(&mut self) -> _ |
| nuzo_dict | LargeDict::with_capacity | 231-245 | pub fn with_capacity(cap: usize) -> Self |
| nuzo_dict | NuzoDict::contains_value | 654-659 | pub fn contains_value(&self, target: &Value) -> bool |
| nuzo_dict | NuzoDict::get | 502-516 | pub fn get(&self, key_index: u32) -> Option |
| nuzo_dict | NuzoDict::get_by_slot | 502-516 | pub fn get_by_slot(&self, slot: usize) -> Option |
| nuzo_dict | NuzoDict::get_with_slot | 537-556 | pub fn get_with_slot(&self, key_index: u32) -> (Option, O... |
| nuzo_dict | NuzoDict::insert | 559-592 | pub fn insert(&mut self, key_index: u32, value: Value) |
| nuzo_dict | NuzoDict::insert_with_slot | 559-592 | pub fn insert_with_slot(&mut self, key_index: u32, value:... |
| nuzo_dict | NuzoDict::is_empty | 626-631 | pub fn is_empty(&self) -> bool |
| nuzo_dict | NuzoDict::iter | 633-638 | pub fn iter(&self) -> Box |
| nuzo_dict | NuzoDict::len | 618-623 | pub fn len(&self) -> usize |
| nuzo_dict | NuzoDict::new | 477-479 | pub fn new() -> Self |
| nuzo_dict | NuzoDict::set_by_slot | 520-533 | pub fn set_by_slot(&mut self, slot: usize, value: Value) |
| nuzo_dict | NuzoDict::shape_id | 484-497 | pub fn shape_id(&self) -> u32 |
| nuzo_dict | NuzoDict::size_estimate | 662-667 | pub fn size_estimate(&self) -> usize |
| nuzo_dict | NuzoDict::values | 640-645 | pub fn values(&self) -> Box |
| nuzo_dict | NuzoDict::values_mut | 647-652 | pub fn values_mut(&mut self) -> Box |
| nuzo_dict | NuzoEntry::new | 115-121 | pub fn new(key_index: u32, value: Value) -> Self |
| nuzo_dict | SmallDict::contains_value | 185-187 | pub fn contains_value(&self, target: &Value) -> bool |
| nuzo_dict | SmallDict::get | 144-151 | pub fn get(&self, key_index: u32) -> Option |
| nuzo_dict | SmallDict::insert | 153-161 | pub fn insert(&mut self, key_index: u32, value: Value) |
| nuzo_dict | SmallDict::into_entries | 189-191 | pub fn into_entries(self) -> Vec |
| nuzo_dict | SmallDict::is_empty | 169-171 | pub fn is_empty(&self) -> bool |
| nuzo_dict | SmallDict::iter | 173-175 | pub fn iter(&self) -> _ |
| nuzo_dict | SmallDict::len | 164-166 | pub fn len(&self) -> usize |
| nuzo_dict | SmallDict::new | 138-142 | pub fn new() -> Self |
| nuzo_dict | SmallDict::values | 177-179 | pub fn values(&self) -> _ |
| nuzo_dict | SmallDict::values_mut | 181-183 | pub fn values_mut(&mut self) -> _ |
| nuzo_dict | nuzo_mix | 75-78 | pub fn nuzo_mix(pool_index: u32) -> u32 |
| nuzo_test_macro | execute_nuzo_source | 50-75 | pub fn execute_nuzo_source(source: &str) -> (Vec, Result) |
| object | Object::get | 374-388 | pub fn get(&mut self, name: &str) -> Option |
| object | Object::has_property | 424-432 | pub fn has_property(&self, name: &str) -> bool |
| object | Object::is_empty | 435 | pub fn is_empty(&self) -> bool |
| object | Object::len | 434 | pub fn len(&self) -> usize |
| object | Object::new | 350-371 | pub fn new(shape: Arc) -> Self |
| object | Object::set | 390-421 | pub fn set(&mut self, name: &str, value: Value) |
| object | Object::shape | 436 | pub fn shape(&self) -> &Arc |
| object | Shape::create | 113-156 | pub fn create(names: &[&str]) -> Arc |
| object | Shape::extend | 207-237 | pub fn extend(&self, new_name: &str) -> Arc |
| object | Shape::find_property | 177-205 | pub fn find_property(&self, name: &str) -> Option |
| object | Shape::is_empty | 240 | pub fn is_empty(&self) -> bool |
| object | Shape::len | 239 | pub fn len(&self) -> usize |
| opcode | Chunk::add_constant | 1125-1140 | pub fn add_constant(&mut self, value: Value) -> usize |
| opcode | Chunk::add_debug_info | 1146-1148 | pub fn add_debug_info(&mut self, ip: usize, line: usize, ... |
| opcode | Chunk::code | 1020 | pub fn code(&self) -> &[u8] |
| opcode | Chunk::code_arc | 1032 | pub fn code_arc(&self) -> &Arc |
| opcode | Chunk::code_mut | 1050 | pub fn code_mut(&mut self) -> &mut Vec |
| opcode | Chunk::constants | 1024 | pub fn constants(&self) -> &[Value] |
| opcode | Chunk::constants_arc | 1036 | pub fn constants_arc(&self) -> &Arc |
| opcode | Chunk::constants_mut | 1054 | pub fn constants_mut(&mut self) -> &mut Vec |
| opcode | Chunk::decode_opcode | 1240 | pub fn decode_opcode(byte: u8) -> Option |
| opcode | Chunk::disassemble | 1266-1297 | pub fn disassemble(&self) -> String |
| opcode | Chunk::emit | 1111 | pub fn emit(&mut self, instr: Instruction) |
| opcode | Chunk::from_arcs | 1067-1081 | pub fn from_arcs(code: Arc, constants: Arc, lines: Arc, d... |
| opcode | Chunk::get_constant | 1143 | pub fn get_constant(&self, idx: usize) -> Option |
| opcode | Chunk::get_source_location | 1151-1165 | pub fn get_source_location(&self, ip: usize) -> Option |
| opcode | Chunk::into_parts | 1085-1087 | pub fn into_parts(self) -> (Arc, Arc, Arc, Arc, u16) |
| opcode | Chunk::is_empty | 1237 | pub fn is_empty(&self) -> bool |
| opcode | Chunk::len | 1234 | pub fn len(&self) -> usize |
| opcode | Chunk::lines | 1028 | pub fn lines(&self) -> &[u32] |
| opcode | Chunk::lines_arc | 1040 | pub fn lines_arc(&self) -> &Arc |
| opcode | Chunk::lines_mut | 1058 | pub fn lines_mut(&mut self) -> &mut Vec |
| opcode | Chunk::new | 1005-1012 | pub fn new() -> Self |
| opcode | Chunk::write_byte | 1093 | pub fn write_byte(&mut self, b: u8) |
| opcode | Chunk::write_i16 | 1105 | pub fn write_i16(&mut self, val: i16) |
| opcode | Chunk::write_opcode | 1090 | pub fn write_opcode(&mut self, op: Opcode) |
| opcode | Chunk::write_u16 | 1099-1102 | pub fn write_u16(&mut self, val: u16) |
| opcode | Instruction::size | 901-903 | pub fn size(&self) -> usize |
| opcode_gen | DispatchKindVal::to_tokens | 58-67 | pub fn to_tokens(&self) -> proc_macro2::TokenStream |
| opcode_gen | OptionDisasm::to_tokens | 40-48 | pub fn to_tokens(&self) -> proc_macro2::TokenStream |
| opcode_gen | expand_define_opcodes | 430-434 | pub fn expand_define_opcodes(input: proc_macro2::TokenStr... |
| opcode_gen | generate_opcode_code | 283-425 | pub fn generate_opcode_code(name: Ident, defs: Vec) -> sy... |
| opcode_gen | parse_opcode_defs | 216-262 | pub fn parse_opcode_defs(items: Vec) -> syn::Result |
| opcode_sync_derive | expand_opcode_sync | 123-161 | pub fn expand_opcode_sync(input: &syn::DeriveInput) -> sy... |
| optimize | optimize | 27-33 | pub fn optimize(module: &mut IrModule) -> usize |
| optimize | optimize_block | 50-114 | pub fn optimize_block(block: &mut BasicBlock) -> usize |
| optimize | optimize_function | 36-42 | pub fn optimize_function(func: &mut IrFunction) -> usize |
| output | Output::first_line | 23-25 | pub fn first_line(&self) -> Option |
| output | Output::stdout_text | 18-20 | pub fn stdout_text(&self) -> String |
| parse_utils | camel_to_snake | 238-254 | pub fn camel_to_snake(s: &str) -> String |
| parse_utils | camel_to_snake_op | 262-264 | pub fn camel_to_snake_op(ident: &str) -> String |
| parse_utils | extract_string_array | 142-166 | pub fn extract_string_array(expr: &Expr) -> syn::Result |
| parse_utils | operand_byte_size | 273-288 | pub fn operand_byte_size(kind_str: &str) -> syn::Result |
| parse_utils | parse_bool_lit | 100-116 | pub fn parse_bool_lit(expr: &Expr) -> syn::Result |
| parse_utils | parse_f64_lit | 119-135 | pub fn parse_f64_lit(expr: &Expr) -> syn::Result |
| parse_utils | parse_ident_path | 206-225 | pub fn parse_ident_path(expr: &Expr) -> syn::Result |
| parse_utils | parse_int_lit | 27-78 | pub fn parse_int_lit<T>(expr: &Expr, field_name: &str, ra... |
| parse_utils | parse_operand_list | 169-203 | pub fn parse_operand_list(expr: &Expr) -> syn::Result |
| parse_utils | parse_string_lit | 81-97 | pub fn parse_string_lit(expr: &Expr) -> syn::Result |
| parser | Parser::parse | 193-214 | pub fn parse(source: &str) -> Result |
| parser | Parser::parse_with_timing | 231-259 | pub fn parse_with_timing(source: &str) -> Result |
| path | parse_bind_args | 96-98 | pub fn parse_bind_args(input: TokenStream) -> syn::Result |
| perf_regression | BenchmarkConfig::new | 214-216 | pub fn new() -> Self |
| perf_regression | BenchmarkConfig::with_params | 219-224 | pub fn with_params(warmup_iterations: usize, sample_size:... |
| perf_regression | BenchmarkResult::deviation_from_target | 165-171 | pub fn deviation_from_target(&self) -> Option |
| perf_regression | BenchmarkResult::meets_target | 150-160 | pub fn meets_target(&self) -> bool |
| perf_regression | ConsoleReporter::generate | 2132-2218 | pub fn generate(&self, results: &[RegressionResult], summ... |
| perf_regression | ConsoleReporter::new | 2116-2118 | pub fn new(verbose: bool) -> Self |
| perf_regression | JsonReporter::generate | 2386-2424 | pub fn generate(results: &[RegressionResult], summary: &S... |
| perf_regression | Summary::from_results | 1972-1999 | pub fn from_results(results: &[RegressionResult]) -> Self |
| perf_regression | bench_cow_capacity_reuse | 1333-1356 | pub fn bench_cow_capacity_reuse(config: BenchmarkConfig) ... |
| perf_regression | bench_e2e_array_operations | 1174-1227 | pub fn bench_e2e_array_operations(config: BenchmarkConfig... |
| perf_regression | bench_e2e_fibonacci | 1056-1109 | pub fn bench_e2e_fibonacci(config: BenchmarkConfig) -> Be... |
| perf_regression | bench_e2e_sum_primes | 1115-1168 | pub fn bench_e2e_sum_primes(config: BenchmarkConfig) -> B... |
| perf_regression | bench_gc_alloc_throughput | 866-920 | pub fn bench_gc_alloc_throughput(config: BenchmarkConfig)... |
| perf_regression | bench_gc_collect_pause | 926-980 | pub fn bench_gc_collect_pause(config: BenchmarkConfig) ->... |
| perf_regression | bench_object_multi_get | 1236-1259 | pub fn bench_object_multi_get(config: BenchmarkConfig) ->... |
| perf_regression | bench_object_property_get | 778-815 | pub fn bench_object_property_get(config: BenchmarkConfig)... |
| perf_regression | bench_object_property_set | 821-856 | pub fn bench_object_property_set(config: BenchmarkConfig)... |
| perf_regression | bench_shape_dedup | 1308-1326 | pub fn bench_shape_dedup(config: BenchmarkConfig) -> Benc... |
| perf_regression | bench_shape_transition_cache | 1266-1301 | pub fn bench_shape_transition_cache(config: BenchmarkConf... |
| perf_regression | bench_uic_get_hit | 1368-1405 | pub fn bench_uic_get_hit(config: BenchmarkConfig) -> Benc... |
| perf_regression | bench_uic_set_hit | 1413-1448 | pub fn bench_uic_set_hit(config: BenchmarkConfig) -> Benc... |
| perf_regression | bench_value_add | 442-462 | pub fn bench_value_add(config: BenchmarkConfig) -> Benchm... |
| perf_regression | bench_value_as_number | 418-437 | pub fn bench_value_as_number(config: BenchmarkConfig) -> ... |
| perf_regression | bench_value_from_number | 395-413 | pub fn bench_value_from_number(config: BenchmarkConfig) -... |
| perf_regression | bench_value_mul | 467-487 | pub fn bench_value_mul(config: BenchmarkConfig) -> Benchm... |
| perf_regression | bench_vm_complex_expression | 557-611 | pub fn bench_vm_complex_expression(config: BenchmarkConfi... |
| perf_regression | bench_vm_constant_loading | 617-698 | pub fn bench_vm_constant_loading(config: BenchmarkConfig)... |
| perf_regression | bench_vm_simple_arithmetic | 497-551 | pub fn bench_vm_simple_arithmetic(config: BenchmarkConfig... |
| perf_regression | bench_vm_stack_operations | 704-768 | pub fn bench_vm_stack_operations(config: BenchmarkConfig)... |
| perf_regression | detect_output_format | 2080-2089 | pub fn detect_output_format() -> OutputFormat |
| perf_regression | run_all_benchmarks | 1492-1539 | pub fn run_all_benchmarks(config: &BenchmarkConfig) -> Vec |
| perf_regression | run_all_benchmarks_with_options | 1562-1630 | pub fn run_all_benchmarks_with_options(config: &Benchmark... |
| perf_regression | run_benchmark | 249-299 | pub fn run_benchmark<F>(id: &str, name: &str, unit: &str,... |
| py_test | expand_py_test | 21-49 | pub fn expand_py_test(source: &LitStr) -> syn::Result |
| report | ConsoleReporter::generate | 273-359 | pub fn generate(&self, results: &[RegressionResult], summ... |
| report | ConsoleReporter::new | 257-259 | pub fn new(verbose: bool) -> Self |
| report | JsonReporter::generate | 527-565 | pub fn generate(results: &[RegressionResult], summary: &S... |
| report | Summary::from_results | 104-131 | pub fn from_results(results: &[RegressionResult]) -> Self |
| report | detect_output_format | 221-230 | pub fn detect_output_format() -> OutputFormat |
| scope | GlobalScope::define | 505-515 | pub fn define(&mut self, name: &str, value: Value) -> usize |
| scope | GlobalScope::get | 543-545 | pub fn get(&self, idx: usize) -> Option |
| scope | GlobalScope::is_empty | 574-576 | pub fn is_empty(&self) -> bool |
| scope | GlobalScope::len | 569-571 | pub fn len(&self) -> usize |
| scope | GlobalScope::names | 590-592 | pub fn names(&self) -> Vec |
| scope | GlobalScope::new | 475-480 | pub fn new() -> Self |
| scope | GlobalScope::resolve | 528-530 | pub fn resolve(&self, name: &str) -> Option |
| scope | GlobalScope::set | 559-566 | pub fn set(&mut self, idx: usize, value: Value) |
| scope | Scope::active_locals | 352-358 | pub fn active_locals(&self) -> Vec |
| scope | Scope::all_locals | 389-391 | pub fn all_locals(&self) -> Vec |
| scope | Scope::all_names | 381-383 | pub fn all_names(&self) -> Vec |
| scope | Scope::begin_scope | 215-217 | pub fn begin_scope(&mut self) |
| scope | Scope::define | 249-259 | pub fn define(&mut self, name: &str, reg: u16) |
| scope | Scope::depth | 196-198 | pub fn depth(&self) -> usize |
| scope | Scope::end_scope | 228-232 | pub fn end_scope(&mut self) |
| scope | Scope::find_name_by_reg | 371-378 | pub fn find_name_by_reg(&self, reg: u16) -> Option |
| scope | Scope::locals_at_depth | 334-340 | pub fn locals_at_depth(&self, depth: usize) -> Vec |
| scope | Scope::new | 183-188 | pub fn new() -> Self |
| scope | Scope::rebind | 408-415 | pub fn rebind(&mut self, name: &str, new_reg: u16) -> bool |
| scope | Scope::resolve | 278-285 | pub fn resolve(&self, name: &str) -> Option |
| scope | Scope::resolve_or_global | 315-320 | pub fn resolve_or_global(&self, name: &str, globals: &Glo... |
| signal | Signal::clone_handle | 803-810 | pub fn clone_handle(&self) -> Self |
| signal | Signal::connect | 263-265 | pub fn connect<F>(&self, slot: F) -> Result |
| signal | Signal::connect_once | 361-363 | pub fn connect_once<F>(&self, slot: F) -> Result |
| signal | Signal::connect_with_group | 326-332 | pub fn connect_with_group<F>(&self, slot: F, group: &str)... |
| signal | Signal::connect_with_priority | 291-297 | pub fn connect_with_priority<F>(&self, slot: F, priority:... |
| signal | Signal::disconnect_all | 699-712 | pub fn disconnect_all(&self) |
| signal | Signal::disconnect_by_group | 733-752 | pub fn disconnect_by_group(&self, group: &str) |
| signal | Signal::emit | 480-482 | pub fn emit(&self, args: &Args) -> EmitResult |
| signal | Signal::emit_with_options | 526-682 | pub fn emit_with_options(&self, args: &Args, options: Emi... |
| signal | Signal::is_empty | 763-765 | pub fn is_empty(&self) -> bool |
| signal | Signal::name | 229-236 | pub fn name(&self) -> &str |
| signal | Signal::named | 229-236 | pub fn named(name: &str) -> Self |
| signal | Signal::slot_count | 784-786 | pub fn slot_count(&self) -> usize |
| slot | Connection::disconnect | 385-395 | pub fn disconnect(&self) |
| slot | Connection::id | 318-320 | pub fn id(&self) -> ConnectionId |
| slot | Connection::is_connected | 351-353 | pub fn is_connected(&self) -> bool |
| slot | Connection::signal_name | 330-332 | pub fn signal_name(&self) -> &str |
| slot | SlotEntry::new | 162-178 | pub fn new(id: ConnectionId, callback: Arc, priority: Pri... |
| source_location | SourceLocation::new | 131-139 | pub fn new(line: usize) -> Self |
| source_location | SourceLocation::with_column | 142-145 | pub fn with_column(self, column: usize) -> Self |
| source_location | SourceLocation::with_function | 148-151 | pub fn with_function(self, name: _) -> Self |
| source_location | SourceLocation::with_source_line | 154-157 | pub fn with_source_line(self, line: _) -> Self |
| statements | Compiler::compile_block | 81-134 | pub fn compile_block(&mut self, statements: &[ast::Stmt],... |
| statistics | cohens_d | 848-899 | pub fn cohens_d(group1: &[f64], group2: &[f64]) -> Result |
| statistics | compute_mean | 116-135 | pub fn compute_mean(data: &[f64]) -> Result |
| statistics | compute_median | 218-234 | pub fn compute_median(data: &[f64]) -> Result |
| statistics | compute_percentile | 265-292 | pub fn compute_percentile(data: &[f64], percentile: f64) ... |
| statistics | compute_stddev | 162-197 | pub fn compute_stddev(data: &[f64]) -> Result |
| statistics | detect_outliers_iqr | 998-1000 | pub fn detect_outliers_iqr(data: &[f64]) -> Result |
| statistics | detect_outliers_iqr_with_factor | 1012-1045 | pub fn detect_outliers_iqr_with_factor(data: &[f64], k: f... |
| statistics | welch_t_test | 407-468 | pub fn welch_t_test(group1: &[f64], group2: &[f64]) -> Re... |
| string | register | 113-126 | pub fn register(reg: &mut BuiltinRegistry) |
| tag_registry | TagRegistry::all_tags | 235-237 | pub fn all_tags() -> &[TagDescriptor] |
| tag_registry | TagRegistry::check_conflict | 267-289 | pub fn check_conflict(new_tag: u64, new_mask: u64) -> Result |
| tag_registry | TagRegistry::check_conflict_named | 316-322 | pub fn check_conflict_named(new_name: &str, new_tag: u64,... |
| tag_registry | TagRegistry::classify_bits | 328-330 | pub fn classify_bits(bits: u64) -> Option |
| tag_registry | TagRegistry::find_by_name | 246-248 | pub fn find_by_name(name: &str) -> Option |
| tag_registry | TagRegistry::find_by_value_tag | 254-256 | pub fn find_by_value_tag(tag: ValueTag) -> Option |
| tag_registry | TagRegistry::tag_count | 334-336 | pub fn tag_count() -> usize |
| test_attr | expand_nuzo_test_attr | 75-202 | pub fn expand_nuzo_test_attr(item: &ItemFn, input: NuzoTe... |
| test_attr | parse_nuzo_test_attrs | 19-72 | pub fn parse_nuzo_test_attrs(meta_list: &[syn::Meta]) -> ... |
| throughput_bench | bench_dict_operations | 1003-1024 | pub fn bench_dict_operations(config: &BenchmarkConfig) ->... |
| throughput_bench | bench_fibonacci | 850-871 | pub fn bench_fibonacci(config: &BenchmarkConfig) -> Bench... |
| throughput_bench | bench_function_call | 970-993 | pub fn bench_function_call(config: &BenchmarkConfig) -> B... |
| throughput_bench | bench_numeric_loop | 880-900 | pub fn bench_numeric_loop(config: &BenchmarkConfig) -> Be... |
| throughput_bench | bench_property_access | 910-931 | pub fn bench_property_access(config: &BenchmarkConfig) ->... |
| throughput_bench | bench_string_concat | 941-960 | pub fn bench_string_concat(config: &BenchmarkConfig) -> B... |
| throughput_bench | run_all_throughput_benchmarks | 1031-1040 | pub fn run_all_throughput_benchmarks(config: &BenchmarkCo... |
| time | register | 78-83 | pub fn register(reg: &mut BuiltinRegistry) |
| timeout | __timeout_alarm_inner | 144-196 | pub fn __timeout_alarm_inner<T, F>(timeout: std::time::Du... |
| timeout_alarm | __timeout_alarm_inner | 144-196 | pub fn __timeout_alarm_inner<T, F>(timeout: std::time::Du... |
| token | Token::eof | 267-269 | pub fn eof(line: usize, column: usize, offset: usize) -> ... |
| token | Token::new | 250-252 | pub fn new(kind: TokenKind, line: usize, column: usize, o... |
| token | TokenBuilder::build | 65-67 | pub fn build(self) -> TokenStream |
| token | TokenBuilder::group | 53-56 | pub fn group(self, delimiter: Delimiter, inner: TokenStre... |
| token | TokenBuilder::ident | 29-32 | pub fn ident(self, name: &str) -> Self |
| token | TokenBuilder::is_empty | 76-78 | pub fn is_empty(&self) -> bool |
| token | TokenBuilder::len | 71-73 | pub fn len(&self) -> usize |
| token | TokenBuilder::literal | 47-50 | pub fn literal(self, lit: Literal) -> Self |
| token | TokenBuilder::new | 24-26 | pub fn new() -> Self |
| token | TokenBuilder::punct | 35-38 | pub fn punct(self, ch: char) -> Self |
| token | TokenBuilder::punct_with_spacing | 41-44 | pub fn punct_with_spacing(self, ch: char, spacing: Spacin... |
| token | TokenBuilder::token_stream | 59-62 | pub fn token_stream(self, ts: TokenStream) -> Self |
| token | TokenKind::is_and | 566 | pub fn is_and(self) -> bool |
| token | TokenKind::is_break | 538 | pub fn is_break(self) -> bool |
| token | TokenKind::is_continue | 541 | pub fn is_continue(self) -> bool |
| token | TokenKind::is_else | 523 | pub fn is_else(self) -> bool |
| token | TokenKind::is_false | 553 | pub fn is_false(self) -> bool |
| token | TokenKind::is_fn | 544 | pub fn is_fn(self) -> bool |
| token | TokenKind::is_for | 529 | pub fn is_for(self) -> bool |
| token | TokenKind::is_if | 520 | pub fn is_if(self) -> bool |
| token | TokenKind::is_in | 532 | pub fn is_in(self) -> bool |
| token | TokenKind::is_loop | 535 | pub fn is_loop(self) -> bool |
| token | TokenKind::is_nil | 556 | pub fn is_nil(self) -> bool |
| token | TokenKind::is_or | 572 | pub fn is_or(self) -> bool |
| token | TokenKind::is_return | 547 | pub fn is_return(self) -> bool |
| token | TokenKind::is_true | 550 | pub fn is_true(self) -> bool |
| token | TokenKind::is_while | 526 | pub fn is_while(self) -> bool |
| token | TokenParser::expect_ident | 150-159 | pub fn expect_ident(&mut self) -> syn::Result |
| token | TokenParser::expect_literal | 184-193 | pub fn expect_literal(&mut self) -> syn::Result |
| token | TokenParser::expect_punct | 165-178 | pub fn expect_punct(&mut self, ch: char) -> syn::Result |
| token | TokenParser::is_empty | 127-129 | pub fn is_empty(&self) -> bool |
| token | TokenParser::new | 119-124 | pub fn new(input: TokenStream) -> Self |
| token | TokenParser::parse | 202-220 | pub fn parse<T>(&mut self) -> syn::Result |
| token | TokenParser::peek_ident | 132-134 | pub fn peek_ident(&self) -> bool |
| token | TokenParser::peek_literal | 142-144 | pub fn peek_literal(&self) -> bool |
| token | TokenParser::peek_punct | 137-139 | pub fn peek_punct(&self, ch: char) -> bool |
| token | TokenParser::remaining | 223-225 | pub fn remaining(&self) -> TokenStream |
| token | parse_comma_separated | 282-302 | pub fn parse_comma_separated<T>(input: ParseStream) -> sy... |
| token | peek_and_consume | 269-277 | pub fn peek_and_consume(input: ParseStream, punct: char) ... |
| trace_derive | expand_trace | 194-284 | pub fn expand_trace(input: &DeriveInput) -> syn::Result |
| tracer | Tracer::connect_signals | 77-118 | pub fn connect_signals(&self, will_execute: &Signal, vm_e... |
| tracer | Tracer::disconnect_signals | 120-127 | pub fn disconnect_signals(&self) |
| tracer | Tracer::new | 68-75 | pub fn new() -> Self |
| tracer | Tracer::run_with_trace | 145-176 | pub fn run_with_trace(source: &str, config: TraceConfig) ... |
| tracer | Tracer::take_errors | 137-143 | pub fn take_errors(&self) -> Vec |
| tracer | Tracer::take_trace_entries | 129-135 | pub fn take_trace_entries(&self) -> Vec |
| tracer_state | TraceResult::entries_for_opcode | 137-142 | pub fn entries_for_opcode(&self, opcode: Opcode) -> Vec |
| tracer_state | TraceResult::filtered_count | 101-103 | pub fn filtered_count(&self) -> usize |
| tracer_state | TraceResult::format_trace | 105-127 | pub fn format_trace(&self) -> String |
| tracer_state | TraceResult::max_frame_depth | 129-135 | pub fn max_frame_depth(&self) -> usize |
| tracer_state | TracerState::instruction_counter | 177-179 | pub fn instruction_counter(&self) -> usize |
| tracer_state | TracerState::into_result | 282-289 | pub fn into_result(self, total_instructions: usize) -> Tr... |
| tracer_state | TracerState::new | 154-162 | pub fn new(config: TraceConfig) -> Self |
| tracer_state | TracerState::record | 187-231 | pub fn record(&mut self, opcode: Opcode, operands: Vec, i... |
| tracer_state | TracerState::should_capture_registers | 182-184 | pub fn should_capture_registers(&self) -> bool |
| tracer_state | TracerState::should_record | 165-174 | pub fn should_record(&self, opcode: &Opcode) -> bool |
| trf | TypedRegFile::activate | 639 | pub fn activate(&self) |
| trf | TypedRegFile::as_slice_data | 636 | pub fn as_slice_data(&self) -> &[u64] |
| trf | TypedRegFile::as_slice_tags | 637 | pub fn as_slice_tags(&self) -> &[u8] |
| trf | TypedRegFile::capacity | 604 | pub fn capacity(&self) -> usize |
| trf | TypedRegFile::clear | 632 | pub fn clear(&mut self) |
| trf | TypedRegFile::copy_within | 633 | pub fn copy_within(&mut self, src: std::ops::Range, dest_... |
| trf | TypedRegFile::deactivate | 640 | pub fn deactivate(&self) |
| trf | TypedRegFile::first | 634 | pub fn first(&self) -> Option |
| trf | TypedRegFile::is_empty | 603 | pub fn is_empty(&self) -> bool |
| trf | TypedRegFile::len | 602 | pub fn len(&self) -> usize |
| trf | TypedRegFile::new | 600 | pub fn new() -> Self |
| trf | TypedRegFile::pop | 629 | pub fn pop(&mut self) -> Option |
| trf | TypedRegFile::push | 627 | pub fn push(&mut self, val: u64, tag: RegTag) |
| trf | TypedRegFile::push_value | 628 | pub fn push_value(&mut self, val: Value) |
| trf | TypedRegFile::resize | 631 | pub fn resize(&mut self, new_len: usize, fill_val: u64, f... |
| trf | TypedRegFile::retag_range | 625 | pub fn retag_range(&mut self, start: usize, end: usize) |
| trf | TypedRegFile::set_tagged | 622 | pub fn set_tagged(&mut self, idx: usize, val: u64, tag: R... |
| trf | TypedRegFile::set_value | 623 | pub fn set_value(&mut self, idx: usize, val: Value) |
| trf | TypedRegFile::truncate | 630 | pub fn truncate(&mut self, new_len: usize) |
| trf | TypedRegFileInner::activate | 400 | pub fn activate(&self) |
| trf | TypedRegFileInner::as_slice_data | 333 | pub fn as_slice_data(&self) -> &[u64] |
| trf | TypedRegFileInner::as_slice_tags | 334 | pub fn as_slice_tags(&self) -> &[u8] |
| trf | TypedRegFileInner::capacity | 162 | pub fn capacity(&self) -> usize |
| trf | TypedRegFileInner::clear | 304-312 | pub fn clear(&mut self) |
| trf | TypedRegFileInner::copy_within | 314-331 | pub fn copy_within(&mut self, src: std::ops::Range, dest_... |
| trf | TypedRegFileInner::deactivate | 401 | pub fn deactivate(&self) |
| trf | TypedRegFileInner::first | 336-340 | pub fn first(&self) -> Option |
| trf | TypedRegFileInner::infer_tag_from_value | 219 | pub fn infer_tag_from_value(val: Value) -> RegTag |
| trf | TypedRegFileInner::is_empty | 161 | pub fn is_empty(&self) -> bool |
| trf | TypedRegFileInner::len | 160 | pub fn len(&self) -> usize |
| trf | TypedRegFileInner::new | 98-153 | pub fn new(reserve_slots: usize, initial_slots: usize) ->... |
| trf | TypedRegFileInner::pop | 270-274 | pub fn pop(&mut self) -> Option |
| trf | TypedRegFileInner::push | 267 | pub fn push(&mut self, val: u64, tag: RegTag) |
| trf | TypedRegFileInner::push_value | 268 | pub fn push_value(&mut self, val: Value) |
| trf | TypedRegFileInner::resize | 290-302 | pub fn resize(&mut self, new_len: usize, fill_val: u64, f... |
| trf | TypedRegFileInner::set_tagged | 188-198 | pub fn set_tagged(&mut self, idx: usize, val: u64, tag: R... |
| trf | TypedRegFileInner::set_value | 215-217 | pub fn set_value(&mut self, idx: usize, val: Value) |
| trf | TypedRegFileInner::truncate | 276-288 | pub fn truncate(&mut self, new_len: usize) |
| types | BasicBlock::is_valid | 212-215 | pub fn is_valid(&self) -> bool |
| types | BasicBlock::new | 202-204 | pub fn new(id: BasicBlockId) -> Self |
| types | BasicBlock::push | 207-209 | pub fn push(&mut self, op: IrOp) |
| types | BasicBlockId::new | 24 | pub fn new(id: u32) -> Self |
| types | ConnectionId::as_u64 | 47-49 | pub fn as_u64(&self) -> u64 |
| types | EmitResult::is_ok | 244-246 | pub fn is_ok(&self) -> bool |
| types | ExecutionContext::add_register | 135-139 | pub fn add_register(&mut self, idx: usize, value: Value) |
| types | ExecutionContext::new | 111-120 | pub fn new(ip: usize, opcode: Option, call_depth: usize) ... |
| types | ExecutionContext::operands | 142-144 | pub fn operands(&mut self, regs: Vec) |
| types | ExecutionContext::source_location | 147-149 | pub fn source_location(&mut self, loc: SourceLocation) |
| types | ExecutionContext::with_source | 123-132 | pub fn with_source(ip: usize, opcode: Option, call_depth:... |
| types | IrBinOp::as_str | 57-64 | pub fn as_str(self) -> &str |
| types | IrFunction::current_block_mut | 256-258 | pub fn current_block_mut(&mut self) -> &mut BasicBlock |
| types | IrFunction::new | 241-253 | pub fn new(id: IrFunctionId, name: _) -> Self |
| types | IrFunctionId::new | 32 | pub fn new(id: u32) -> Self |
| types | IrModule::add_function | 282-287 | pub fn add_function(&mut self, name: _) -> IrFunctionId |
| types | IrModule::current_function_mut | 290-292 | pub fn current_function_mut(&mut self) -> &mut IrFunction |
| types | IrModule::new | 277-279 | pub fn new() -> Self |
| types | IrOp::dest | 163-182 | pub fn dest(&self) -> Option |
| types | IrOp::is_terminator | 155-160 | pub fn is_terminator(&self) -> bool |
| types | IrUnaryOp::as_str | 75-77 | pub fn as_str(self) -> &str |
| types | Priority::order_value | 110-116 | pub fn order_value(&self) -> i32 |
| types | StackFrameInfo::call_site | 275-277 | pub fn call_site(&mut self, site: SourceLocation) |
| types | StackFrameInfo::ip_range | 264-266 | pub fn ip_range(&mut self, start: usize, end: usize) |
| types | StackFrameInfo::new | 252-261 | pub fn new(function_name: String, base_register: usize) -... |
| types | StackFrameInfo::source | 269-272 | pub fn source(&mut self, file: String, line: usize) |
| types | ValueRef::new | 15 | pub fn new(id: u32) -> Self |
| util | decode_source_bytes | 105-126 | pub fn decode_source_bytes(bytes: &[u8]) -> Result |
| util | format_bytes | 129-141 | pub fn format_bytes(bytes: usize) -> String |
| util | is_utf16_bom | 63-66 | pub fn is_utf16_bom(b0: u8, b1: u8) -> bool |
| util | is_utf8_bom | 70-72 | pub fn is_utf8_bom(b0: u8, b1: u8, b2: u8) -> bool |
| util | read_source_file | 97-102 | pub fn read_source_file(path: &str) -> Result |
| util | strip_bom | 79-87 | pub fn strip_bom(bytes: &[u8]) -> &[u8] |
| validate | collect_field_names | 105-112 | pub fn collect_field_names(fields: &syn::Fields) -> Vec |
| validate | generate_where_clause | 56-72 | pub fn generate_where_clause(generics: &syn::Generics, bo... |
| validate | validate_field_types | 31-54 | pub fn validate_field_types(fields: &syn::Fields, allowed... |
| validate | validate_ident_not_reserved | 74-87 | pub fn validate_ident_not_reserved(ident: &proc_macro2::I... |
| validate | validate_no_duplicate_attrs | 15-29 | pub fn validate_no_duplicate_attrs(attrs: &[syn::Attribut... |
| validate | validate_vis | 89-103 | pub fn validate_vis(vis: &syn::Visibility, expected: VisK... |
| value | Value::add | 823-834 | pub fn add(self, other: Value) -> Result |
| value | Value::as_bool | 801 | pub fn as_bool(self) -> bool |
| value | Value::as_builtin_fn_opt | 1085-1091 | pub fn as_builtin_fn_opt(self) -> Option |
| value | Value::as_closure_heap_object_opt | 1077-1079 | pub fn as_closure_heap_object_opt(self) -> Option |
| value | Value::as_closure_opt | 1069-1075 | pub fn as_closure_opt(self) -> Option |
| value | Value::as_heap_object_opt | 467-495 | pub fn as_heap_object_opt(self) -> Option |
| value | Value::as_heap_object_ref | 677-695 | pub fn as_heap_object_ref(&self) -> Option |
| value | Value::as_number | 787-790 | pub fn as_number(self) -> f64 |
| value | Value::as_ptr | 814-816 | pub fn as_ptr(self) -> *const u8 |
| value | Value::as_range_opt | 1053-1063 | pub fn as_range_opt(self) -> Option |
| value | Value::as_smi | 751-755 | pub fn as_smi(self) -> i64 |
| value | Value::as_string_opt | 988-993 | pub fn as_string_opt(self) -> Option |
| value | Value::collection_contains | 431-442 | pub fn collection_contains(self, target: Value) -> bool |
| value | Value::concat_repr | 1041-1047 | pub fn concat_repr(self) -> String |
| value | Value::div | 881-888 | pub fn div(self, other: Value) -> Result |
| value | Value::from_arena_index | 628-634 | pub fn from_arena_index(offset: u32) -> Value |
| value | Value::from_bool | 800 | pub fn from_bool(b: bool) -> Value |
| value | Value::from_gc_index | 592-596 | pub fn from_gc_index(idx: u32) -> Value |
| value | Value::from_heap_object_gc | 580-585 | pub fn from_heap_object_gc(obj: HeapObject) -> Value |
| value | Value::from_number | 776-784 | pub fn from_number(n: f64) -> Value |
| value | Value::from_scratch_index | 603-609 | pub fn from_scratch_index(idx: u32) -> Value |
| value | Value::from_smi | 738-741 | pub fn from_smi(i: i64) -> Value |
| value | Value::from_string | 940-976 | pub fn from_string(s: &str) -> Value |
| value | Value::from_string_index | 999-1001 | pub fn from_string_index(idx: u32) -> Value |
| value | Value::heap_idx_or_err | 543-552 | pub fn heap_idx_or_err(self) -> Result |
| value | Value::heap_index | 528-530 | pub fn heap_index(self) -> Option |
| value | Value::is_arena_index | 641-643 | pub fn is_arena_index(idx: u32) -> bool |
| value | Value::is_bool | 410 | pub fn is_bool(self) -> bool |
| value | Value::is_builtin_fn | 1081-1083 | pub fn is_builtin_fn(self) -> bool |
| value | Value::is_callable | 1093 | pub fn is_callable(self) -> bool |
| value | Value::is_closure | 1065-1067 | pub fn is_closure(self) -> bool |
| value | Value::is_collection | 419-429 | pub fn is_collection(self) -> bool |
| value | Value::is_float | 400 | pub fn is_float(self) -> bool |
| value | Value::is_gc_managed | 554-556 | pub fn is_gc_managed(self) -> bool |
| value | Value::is_heap_object | 464 | pub fn is_heap_object(self) -> bool |
| value | Value::is_nil | 411 | pub fn is_nil(self) -> bool |
| value | Value::is_number | 403-408 | pub fn is_number(self) -> bool |
| value | Value::is_ptr | 459-462 | pub fn is_ptr(self) -> bool |
| value | Value::is_range | 1049-1051 | pub fn is_range(self) -> bool |
| value | Value::is_scratch_index | 578 | pub fn is_scratch_index(idx: u32) -> bool |
| value | Value::is_smi | 397 | pub fn is_smi(self) -> bool |
| value | Value::is_special | 456 | pub fn is_special(self) -> bool |
| value | Value::is_string | 731 | pub fn is_string(self) -> bool |
| value | Value::is_truthy | 413-417 | pub fn is_truthy(self) -> bool |
| value | Value::modulo | 912-918 | pub fn modulo(self, other: Value) -> Result |
| value | Value::mul | 862-868 | pub fn mul(self, other: Value) -> Result |
| value | Value::mutate_heap_object | 497-526 | pub fn mutate_heap_object<F, R>(&self, f: F) -> Option |
| value | Value::neg | 927-933 | pub fn neg(self) -> Result |
| value | Value::pow | 920-925 | pub fn pow(self, other: Value) -> Result |
| value | Value::rem | 900-910 | pub fn rem(self, other: Value) -> Result |
| value | Value::string_from_index | 1003-1006 | pub fn string_from_index(idx: u32) -> Option |
| value | Value::string_index | 995-997 | pub fn string_index(self) -> Option |
| value | Value::sub | 845-851 | pub fn sub(self, other: Value) -> Result |
| value | Value::tag | 1029-1037 | pub fn tag(self) -> ValueTag |
| value | Value::to_string_repr | 1039 | pub fn to_string_repr(self) -> String |
| value | Value::try_arena_offset | 650-654 | pub fn try_arena_offset(&self) -> Option |
| value | Value::try_from_raw_bits | 121-129 | pub fn try_from_raw_bits(bits: u64) -> Option |
| value | Value::try_from_smi | 744-748 | pub fn try_from_smi(i: i64) -> Option |
| value | Value::try_number | 792-794 | pub fn try_number(self) -> Option |
| value | Value::try_remap | 661-675 | pub fn try_remap(&mut self, remap: &[(u32, u32)]) -> bool |
| value | Value::type_name | 1012-1026 | pub fn type_name(self) -> &str |
| value | Value::value_equals | 446-454 | pub fn value_equals(self, other: &Value) -> bool |
| value | Value::with_heap_object | 697-717 | pub fn with_heap_object<F, R>(&self, f: F) -> Option |
| value | Value::with_heap_object_mut | 719-729 | pub fn with_heap_object_mut<F, R>(&self, f: F) -> Option |
| value | allocate_box | 366-369 | pub fn allocate_box(value: Value) -> Result |
| value | default_heap_alloc | 282-317 | pub fn default_heap_alloc(obj: HeapObject) -> u32 |
| value | default_heap_get | 323-329 | pub fn default_heap_get(idx: u32) -> *const HeapObject |
| value | default_heap_get_mut | 331-340 | pub fn default_heap_get_mut(idx: u32) -> *mut HeapObject |
| value | get_box | 371-377 | pub fn get_box(idx: usize) -> Option |
| value | get_heap_roots_fn | 358-360 | pub fn get_heap_roots_fn() -> Option |
| value | register_heap_accessors | 342-349 | pub fn register_heap_accessors(alloc_fn: HeapAllocFn, get... |
| value | reset_heap_accessors | 351-356 | pub fn reset_heap_accessors() |
| value | set_box | 379-390 | pub fn set_box(idx: usize, value: Value) -> Result |
| vm | ExecutionContext::new | 436-454 | pub fn new() -> Self |
| vm | ExecutionContext::reset_registers_and_frames | 474-481 | pub fn reset_registers_and_frames(&mut self, locals_count... |
| vm | ExecutionContext::snapshot_for_chunk_switch | 465-471 | pub fn snapshot_for_chunk_switch(&mut self) |
| vm | VM::add_global | 1759 | pub fn add_global(&mut self, value: Value) -> usize |
| vm | VM::build_call_stack_for_debug | 1644-1665 | pub fn build_call_stack_for_debug(&self) -> Option |
| vm | VM::call_depth | 1064-1066 | pub fn call_depth(&self) -> usize |
| vm | VM::clear_diagnostics | 748 | pub fn clear_diagnostics(&mut self) |
| vm | VM::clear_stack | 942 | pub fn clear_stack(&mut self) |
| vm | VM::current_ip | 1630-1632 | pub fn current_ip(&self) -> usize |
| vm | VM::define_global | 1762 | pub fn define_global(&mut self, name: &str, value: Value)... |
| vm | VM::diagnose_internal_error | 808-836 | pub fn diagnose_internal_error(&self, error: &InternalErr... |
| vm | VM::diagnostic_error_count | 746 | pub fn diagnostic_error_count(&self) -> usize |
| vm | VM::disable_diagnostic_mode | 737 | pub fn disable_diagnostic_mode(&mut self) |
| vm | VM::enable_diagnostic_mode | 736 | pub fn enable_diagnostic_mode(&mut self) |
| vm | VM::error_collector | 743 | pub fn error_collector(&self) -> &ErrorCollector |
| vm | VM::error_collector_mut | 744 | pub fn error_collector_mut(&mut self) -> &mut ErrorCollector |
| vm | VM::frame_pager_stats | 1069-1071 | pub fn frame_pager_stats(&self) -> &crate::frame_paging::... |
| vm | VM::gc | 638 | pub fn gc(&self) -> &Gc |
| vm | VM::gc_mut | 639 | pub fn gc_mut(&mut self) -> &mut Gc |
| vm | VM::get_global | 1757 | pub fn get_global(&self, idx: usize) -> Option |
| vm | VM::get_global_by_name | 1763 | pub fn get_global_by_name(&self, name: &str) -> Option |
| vm | VM::global_count | 1760 | pub fn global_count(&self) -> usize |
| vm | VM::global_names | 1826-1828 | pub fn global_names(&self) -> Vec |
| vm | VM::has_diagnostic_errors | 747 | pub fn has_diagnostic_errors(&self) -> bool |
| vm | VM::hot_trace_events | 649-651 | pub fn hot_trace_events(&self) -> &[crate::vm_hot_trace::... |
| vm | VM::init_gc | 580-582 | pub fn init_gc(gc: Gc) -> Self |
| vm | VM::instruction_count | 1636-1638 | pub fn instruction_count(&self) -> usize |
| vm | VM::is_diagnostic_mode | 738 | pub fn is_diagnostic_mode(&self) -> bool |
| vm | VM::is_running | 635-637 | pub fn is_running(&self) -> bool |
| vm | VM::last_call_stack | 753-755 | pub fn last_call_stack(&self) -> &[nuzo_error::StackFrame... |
| vm | VM::local_info | 1854-1871 | pub fn local_info(&self) -> Vec |
| vm | VM::lookup_global | 1805-1807 | pub fn lookup_global(&self, name: &str) -> Option |
| vm | VM::new | 576-578 | pub fn new() -> Self |
| vm | VM::new_with_output_capture | 584-593 | pub fn new_with_output_capture() -> (Self, Arc) |
| vm | VM::new_with_output_capture_and_tracer | 595-607 | pub fn new_with_output_capture_and_tracer(config: crate::... |
| vm | VM::peek | 935-939 | pub fn peek(&self, offset: usize) -> Result |
| vm | VM::pending_exception | 962-964 | pub fn pending_exception(&self) -> Option |
| vm | VM::pop | 931-933 | pub fn pop(&mut self) -> Result |
| vm | VM::pop_frame | 1030-1061 | pub fn pop_frame(&mut self) -> Result |
| vm | VM::print_diagnostic_report | 745 | pub fn print_diagnostic_report(&self) |
| vm | VM::push | 925-929 | pub fn push(&mut self, value: Value) -> Result |
| vm | VM::push_frame | 987-1009 | pub fn push_frame(&mut self, closure: Option, argc: usize... |
| vm | VM::push_frame_with_base | 1011-1028 | pub fn push_frame_with_base(&mut self, return_address: us... |
| vm | VM::resolve_global | 1761 | pub fn resolve_global(&self, name: &str) -> Option |
| vm | VM::run | 1225-1233 | pub fn run(&mut self, chunk: Chunk) -> Result |
| vm | VM::set_global | 1758 | pub fn set_global(&mut self, idx: usize, value: Value) ->... |
| vm | VM::set_global_by_name | 1780-1782 | pub fn set_global_by_name(&mut self, name: &str, value: V... |
| vm | VM::stack_size | 941 | pub fn stack_size(&self) -> usize |
| vm | VM::take_tracer_result | 1622-1624 | pub fn take_tracer_result(&mut self) -> Option |
| vm | VM::with_max_diagnostic_errors | 741 | pub fn with_max_diagnostic_errors(&mut self, max: usize) |
| vm | VM::with_stop_on_fatal | 742 | pub fn with_stop_on_fatal(&mut self, stop: bool) |
| vm | vm_error_signal | 86-88 | pub fn vm_error_signal() -> &Signal |
| vm | vm_will_execute_signal | 82-84 | pub fn vm_will_execute_signal() -> &Signal |
| vm_hot_trace | HotTraceEntry::end_ip | 250 | pub fn end_ip(&self) -> usize |
| vm_hot_trace | HotTraceEntry::hash | 238 | pub fn hash(&self) -> u64 |
| vm_hot_trace | HotTraceEntry::hit_count | 254 | pub fn hit_count(&self) -> u32 |
| vm_hot_trace | HotTraceEntry::is_hot | 262 | pub fn is_hot(&self) -> bool |
| vm_hot_trace | HotTraceEntry::length | 246 | pub fn length(&self) -> u8 |
| vm_hot_trace | HotTraceEntry::start_ip | 242 | pub fn start_ip(&self) -> usize |
| vm_hot_trace | HotTraceEntry::status | 258 | pub fn status(&self) -> TraceStatus |
| vm_hot_trace | HotTraceTable::check | 555-569 | pub fn check(&self, ip: usize) -> Option |
| vm_hot_trace | HotTraceTable::clear | 1090-1100 | pub fn clear(&mut self) |
| vm_hot_trace | HotTraceTable::compute_sequence_hash | 795-812 | pub fn compute_sequence_hash(code: &[u8], start: usize, l... |
| vm_hot_trace | HotTraceTable::config | 981-983 | pub fn config(&self) -> &HotTraceConfig |
| vm_hot_trace | HotTraceTable::find_by_hash | 993-995 | pub fn find_by_hash(&self, hash: u64) -> Option |
| vm_hot_trace | HotTraceTable::get_fused_entry | 1126-1130 | pub fn get_fused_entry(&self, _ip: usize) -> Option |
| vm_hot_trace | HotTraceTable::hot_trace_end | 593-599 | pub fn hot_trace_end(&self, ip: usize) -> usize |
| vm_hot_trace | HotTraceTable::invalidate_fused_cache_for_cigc | 1157-1160 | pub fn invalidate_fused_cache_for_cigc(&mut self, _name_i... |
| vm_hot_trace | HotTraceTable::invalidate_fused_cache_for_csts | 1171-1174 | pub fn invalidate_fused_cache_for_csts(&mut self, _call_s... |
| vm_hot_trace | HotTraceTable::invalidate_fused_cache_for_shape | 1143-1146 | pub fn invalidate_fused_cache_for_shape(&mut self, _shape... |
| vm_hot_trace | HotTraceTable::is_hot_trace | 578-586 | pub fn is_hot_trace(&self, ip: usize) -> bool |
| vm_hot_trace | HotTraceTable::mark_hot | 851-879 | pub fn mark_hot(&mut self, ip: usize, hash: u64, length: ... |
| vm_hot_trace | HotTraceTable::new | 476-478 | pub fn new() -> Self |
| vm_hot_trace | HotTraceTable::profile | 651-748 | pub fn profile(&mut self, ip: usize, opcode: Opcode) |
| vm_hot_trace | HotTraceTable::stats | 965-978 | pub fn stats(&self) -> (u64, u64, usize, usize) |
| vm_hot_trace | HotTraceTable::top_pairs | 1045-1079 | pub fn top_pairs(&self, n: usize) -> Vec |
| vm_hot_trace | HotTraceTable::try_register_at_ip | 891-938 | pub fn try_register_at_ip(&mut self, code: &[u8], ip: usize) |
| vm_hot_trace | HotTraceTable::with_config | 497-511 | pub fn with_config(config: HotTraceConfig) -> Self |
| vm_lic | CallSite::fast_dispatch | 527-596 | pub fn fast_dispatch(&mut self, fingerprint: u64) -> Option |
| vm_lic | CallSite::new | 515-517 | pub fn new() -> Self |
| vm_lic | CallSite::reset | 722-729 | pub fn reset(&mut self) |
| vm_lic | CallSite::update_cache | 607-719 | pub fn update_cache(&mut self, fingerprint: u64, target_t... |
| vm_lic | CallSiteState::as_u8 | 81-83 | pub fn as_u8(self) -> u8 |
| vm_lic | CallSiteStats::hit_rate | 443-449 | pub fn hit_rate(&self) -> f64 |
| vm_lic | CallSiteStats::reset | 453-455 | pub fn reset(&mut self) |
| vm_lic | CallSites::ensure | 765-771 | pub fn ensure(&mut self, idx: usize) -> &mut CallSite |
| vm_lic | CallSites::get | 777-779 | pub fn get(&self, ip: usize) -> &CallSite |
| vm_lic | CallSites::get_mut | 777-779 | pub fn get_mut(&mut self, ip: usize) -> &mut CallSite |
| vm_lic | CallSites::get_mut_or_none | 777-779 | pub fn get_mut_or_none(&mut self, ip: usize) -> Option |
| vm_lic | CallSites::is_empty | 809-811 | pub fn is_empty(&self) -> bool |
| vm_lic | CallSites::len | 803-805 | pub fn len(&self) -> usize |
| vm_lic | CallSites::new | 749-751 | pub fn new() -> Self |
| vm_lic | CallSites::reset_all | 893-897 | pub fn reset_all(&mut self) |
| vm_lic | CallSites::resize | 756-758 | pub fn resize(&mut self, size: usize) |
| vm_lic | CallSites::summary | 816-890 | pub fn summary(&self) -> String |
| vm_lic | MegaCallTable::clear | 413-416 | pub fn clear(&mut self) |
| vm_lic | MegaCallTable::from_poly_cache | 402-409 | pub fn from_poly_cache(poly: &PolyCallCache) -> Self |
| vm_lic | MegaCallTable::insert | 379-399 | pub fn insert(&mut self, entry: MegaEntry) |
| vm_lic | MegaCallTable::lookup | 339-354 | pub fn lookup(&self, fingerprint: u64) -> Option |
| vm_lic | MegaCallTable::lookup_mut | 358-372 | pub fn lookup_mut(&mut self, fingerprint: u64) -> Option |
| vm_lic | MegaEntry::from_pic_entry | 300-310 | pub fn from_pic_entry(entry: &PicCallEntry) -> Self |
| vm_lic | MegaEntry::is_empty | 294-296 | pub fn is_empty(&self) -> bool |
| vm_lic | MonoCallCache::clear | 144-146 | pub fn clear(&mut self) |
| vm_lic | MonoCallCache::matches | 129-131 | pub fn matches(&self, fingerprint: u64) -> bool |
| vm_lic | MonoCallCache::matches_value_bits | 138-140 | pub fn matches_value_bits(&self, bits: u64) -> bool |
| vm_lic | PicCallEntry::new | 176-194 | pub fn new(fingerprint: u64, target_type: CallTargetType,... |
| vm_lic | PolyCallCache::clear | 261-264 | pub fn clear(&mut self) |
| vm_lic | PolyCallCache::insert | 240-257 | pub fn insert(&mut self, entry: PicCallEntry) -> Result |
| vm_lic | PolyCallCache::lookup | 215-220 | pub fn lookup(&self, fingerprint: u64) -> Option |
| vm_lic | PolyCallCache::promote_to_front | 228-233 | pub fn promote_to_front(&mut self, idx: usize) |
| vm_lic | fnv_hash_32 | 928-935 | pub fn fnv_hash_32(bytes: &[u8]) -> u32 |
| vm_lic | fnv_hash_64 | 949-956 | pub fn fnv_hash_64(bytes: &[u8]) -> u64 |
| zero_unbox | generic_add_slow | 191-212 | pub fn generic_add_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_div_slow | 268-270 | pub fn generic_div_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_eq_slow | 310-321 | pub fn generic_eq_slow(a: u64, b: u64) -> (u64, RegTag) |
| zero_unbox | generic_mod_slow | 291-293 | pub fn generic_mod_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_mul_slow | 262-264 | pub fn generic_mul_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_neg_slow | 280-287 | pub fn generic_neg_slow(a: u64) -> Result |
| zero_unbox | generic_not_slow | 303-306 | pub fn generic_not_slow(a: u64) -> (u64, RegTag) |
| zero_unbox | generic_ord_slow | 342-353 | pub fn generic_ord_slow(a: u64, b: u64, cmp: _) -> Result |
| zero_unbox | generic_pow_slow | 297-299 | pub fn generic_pow_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_rem_slow | 274-276 | pub fn generic_rem_slow(a: u64, b: u64) -> Result |
| zero_unbox | generic_sub_slow | 256-258 | pub fn generic_sub_slow(a: u64, b: u64) -> Result |
| zero_unbox | is_f64_pair | 65-67 | pub fn is_f64_pair(a: u64, b: u64) -> bool |
| zero_unbox | is_smi_pair | 71-73 | pub fn is_smi_pair(a: u64, b: u64) -> bool |
| zero_unbox | smi_add | 108-117 | pub fn smi_add(a: u64, b: u64) -> Option |
| zero_unbox | smi_div | 146 | pub fn smi_div(_a: u64, _b: u64) -> Option |
| zero_unbox | smi_mod | 154 | pub fn smi_mod(_a: u64, _b: u64) -> Option |
| zero_unbox | smi_mul | 136-142 | pub fn smi_mul(a: u64, b: u64) -> Option |
| zero_unbox | smi_pow | 158 | pub fn smi_pow(_a: u64, _b: u64) -> Option |
| zero_unbox | smi_rem | 150 | pub fn smi_rem(_a: u64, _b: u64) -> Option |
| zero_unbox | smi_result_or_float | 174-181 | pub fn smi_result_or_float(result_f64: f64) -> (u64, RegTag) |
| zero_unbox | smi_sub | 123-130 | pub fn smi_sub(a: u64, b: u64) -> Option |
| zero_unbox | smi_to_i64 | 162-170 | pub fn smi_to_i64(bits: u64) -> i64 |

## 建议

1. 优先覆盖核心模块: `vm`, `compiler`, `bytecode`, `values`
2. 构造函数 (`new`/`default`) 应优先测试
3. 使用 `cargo test -- --ignored` 运行自动生成的测试桩
4. 实现测试后，将对应条目从 `auto_sync_tests.rs` 移除
