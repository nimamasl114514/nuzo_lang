use nuzo_core::XxHashMap;
use serde::Serialize;

use crate::types::ErrorSeverity;
use nuzo_core::SourceLocation;

// ============================================================================
// Smart Diagnostic Engine v2.0 - Advanced Data Structures
// ============================================================================

/// 1. 相似度配置 - 用于控制错误相似度计算的权重参数
#[derive(Debug, Clone)]
pub struct SimilarityConfig {
    /// 类型权重（错误类型匹配的重要性）
    pub type_weight: f64,
    /// 位置权重（源码位置接近程度的重要性）
    pub location_weight: f64,
    /// 上下文权重（调用上下文相似性的重要性）
    pub context_weight: f64,
    /// 时间权重（发生时间接近程度的重要性）
    pub temporal_weight: f64,
    /// 判定为重复错误的阈值
    pub duplicate_threshold: f64,
    /// 判定为相关错误的阈值
    pub related_threshold: f64,
    /// 时间窗口大小（用于时间聚类）
    pub temporal_window: usize,
}

impl Default for SimilarityConfig {
    fn default() -> Self {
        SimilarityConfig {
            type_weight: 0.3,
            location_weight: 0.25,
            context_weight: 0.25,
            temporal_weight: 0.2,
            duplicate_threshold: 0.95,
            related_threshold: 0.7,
            temporal_window: 50,
        }
    }
}

/// 2. 错误序列关系 - 描述多个错误之间的关联模式
#[derive(Debug, Clone, Serialize)]
pub struct ErrorSequence {
    /// 关联的错误ID列表
    pub error_ids: Vec<usize>,
    /// 序列关系类型
    pub relation_type: SequenceRelationType,
    /// 推测强度（0.0-1.0，越高越确定）
    pub speculation_strength: f64,
    /// 观察到的特征列表
    pub observations: Vec<String>,
}

/// 序列关系的具体类型
#[derive(Debug, Clone, Serialize)]
pub enum SequenceRelationType {
    /// 时间聚集（短时间内集中出现）
    TemporalCluster { time_window: usize, error_count: usize },
    /// 同一上下文（相同函数/调用深度）
    SameContext { function_name: String, shared_call_depth: usize },
    /// 重复模式（相同的错误在不同位置重复出现）
    RepeatedPattern { pattern_type: String, locations: Vec<SourceLocation> },
    /// 未知类型
    Unknown,
}

/// 3. 循环模式 - 检测到的错误循环或重复模式
#[derive(Debug, Clone, Serialize)]
pub struct ErrorPattern {
    /// 模式唯一标识符
    pub pattern_id: usize,
    /// 错误模式描述（注意：字段名是 error_pattern 不是 error_type）
    pub error_pattern: String,
    /// 出现次数
    pub occurrence_count: usize,
    /// 涉及的源码位置列表
    pub locations: Vec<SourceLocation>,
    /// 受影响的函数列表
    pub affected_functions: Vec<String>,
    /// 时间范围（起始指令，结束指令）
    pub time_range: (usize, usize),
    /// 模式严重程度
    pub pattern_severity: ErrorSeverity,
    /// 该模式下所有具体的错误变体（如 TypeMismatch 的不同参数组合）
    pub variants: Vec<String>,
}

/// 4. 聚类组 - 将相似错误分组的结果
#[derive(Debug, Clone, Serialize)]
pub struct ErrorCluster {
    /// 聚类唯一标识符
    pub cluster_id: usize,
    /// 聚类名称/描述
    pub name: String,
    /// 该聚类包含的错误ID列表
    pub error_ids: Vec<usize>,
    /// 代表性错误ID（最典型的错误）
    pub representative_error: usize,
    /// 聚类统计信息
    pub cluster_stats: ClusterStatistics,
}

/// 聚类的统计指标
#[derive(Debug, Clone, Serialize)]
pub struct ClusterStatistics {
    /// 聚类内错误总数
    pub size: usize,

    /// 严重程度分布（已实现 ✅）
    /// - 基于 cluster_errors_simple() 内的实际统计
    /// - 格式: { Fatal: 2, Error: 5, Warning: 1 }
    pub severity_distribution: XxHashMap<ErrorSeverity, usize>,

    /// 平均内部相似度 (0.0 - 1.0)
    /// - 基于抽样算法计算（最多10对比较）
    /// - 单元素聚类返回 1.0（完全相同）
    /// - 多元素聚类反映真实的内部一致性
    pub avg_internal_similarity: f64,

    /// 综合风险评分 (0 - 150)
    /// - 计算公式: severity_factor + count_factor
    /// - severity_factor: Fatal=100, Error=75, Warning=40, Info=10
    /// - count_factor: min(count * 10, 50)
    pub risk_score: f64,
}

/// 5. 去重报告 - 错误去重操作的结果摘要
#[derive(Debug, Clone, Serialize)]
pub struct DeduplicationReport {
    /// 原始错误总数
    pub original_count: usize,
    /// 识别出的重复组（每组是互为重复的错误ID列表）
    pub duplicate_groups: Vec<Vec<usize>>,
    /// 去重后剩余的错误数量
    pub remaining_after_dedup: usize,
}

/// 6. 优先级错误项 - 经过优先级排序后的错误条目
#[derive(Debug, Clone, Serialize)]
pub struct PrioritizedError {
    /// 错误ID
    pub error_id: usize,
    /// 优先级排名（越小越重要）
    pub priority: usize,
    /// 综合评分（0.0-1.0，越高越需要优先处理）
    pub score: f64,
    /// 评分明细
    pub score_breakdown: PriorityScoreBreakdown,
    /// 增强版修复建议
    pub enhanced_suggestions: Vec<EnhancedFixSuggestion>,
}

/// 优先级评分的详细分解
#[derive(Debug, Clone, Serialize)]
pub struct PriorityScoreBreakdown {
    /// 严重程度得分
    pub severity_score: f64,
    /// 影响范围得分
    pub impact_score: f64,
    /// 出现频率得分
    pub frequency_score: f64,
    /// 可修复性得分（越容易修复分数越高）
    pub fixability_score: f64,
    /// 上下文重要性得分
    pub context_importance: f64,
}

/// 7. 增强修复建议 - 包含更多元数据的修复建议
#[derive(Debug, Clone, Serialize)]
pub struct EnhancedFixSuggestion {
    /// 建议标题
    pub title: String,
    /// 详细描述
    pub description: String,
    /// 修复难度等级
    pub difficulty: FixDifficulty,
    /// 预估工作量
    pub estimated_effort: EffortLevel,
    /// 是否附带代码示例
    pub has_code_example: bool,
    /// 相关的错误链ID列表
    pub related_chains: Vec<usize>,
}

/// 修复难度枚举
#[derive(Debug, Clone, Serialize)]
pub enum FixDifficulty {
    /// 容易修复
    Easy,
    /// 中等难度
    Medium,
    /// 困难修复
    Hard,
    /// 未知难度
    Unknown,
}

/// 工作量级别枚举
#[derive(Debug, Clone, Serialize)]
pub enum EffortLevel {
    /// 微不足道的工作量
    Trivial,
    /// 小工作量
    Small,
    /// 中等工作量
    Medium,
    /// 大工作量
    Large,
    /// 未知工作量
    Unknown,
}
