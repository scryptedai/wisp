use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HtmlExportOptions {
    pub content: bool,
    pub since: Option<String>,
    pub ids_only: bool,
    pub graph_depth: u32,
    pub graph_max_nodes: u32,
    pub sign: bool,
}

impl HtmlExportOptions {
    pub fn full(content: bool) -> Self {
        Self {
            content,
            since: None,
            ids_only: false,
            graph_depth: DEFAULT_GRAPH_DEPTH,
            graph_max_nodes: DEFAULT_GRAPH_MAX_NODES,
            sign: false,
        }
    }
}

pub const DEFAULT_GRAPH_DEPTH: u32 = 3;
pub const DEFAULT_GRAPH_MAX_NODES: u32 = 48;
pub const HISTORY_PAGE_SIZE: usize = 100;
