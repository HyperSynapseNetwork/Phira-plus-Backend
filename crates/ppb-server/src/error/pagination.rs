//! Pagination helpers (contract: request `page,pageNum(≤100)`, response `{items,total,page,pageNum}`).
//!
//! `page` is a 1-based page index; `pageNum` is the per-page size (see PHASE_A_PLAN P1).

use serde::{Deserialize, Serialize};

use super::{ApiError, ErrorCode};

/// Maximum allowed page size.
pub const MAX_PAGE_NUM: i64 = 100;
/// Default page size.
pub const DEFAULT_PAGE_NUM: i64 = 20;

/// Query-string pagination parameters.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PaginationParams {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default, rename = "pageNum")]
    pub page_num: Option<i64>,
}

impl PaginationParams {
    /// Resolve to (page, page_num), validating pageNum bounds.
    pub fn resolve(&self) -> Result<(i64, i64), ApiError> {
        let page = self.page.unwrap_or(1).max(1);
        let page_num = self.page_num.unwrap_or(DEFAULT_PAGE_NUM);
        if !(1..=MAX_PAGE_NUM).contains(&page_num) {
            return Err(ApiError::new(
                ErrorCode::Pagination,
                format!("pageNum must be between 1 and {MAX_PAGE_NUM}"),
            ));
        }
        Ok((page, page_num))
    }

    /// Resolve to (offset, limit) for SQL OFFSET/LIMIT.
    pub fn offset_limit(&self) -> Result<(i64, i64), ApiError> {
        let (page, page_num) = self.resolve()?;
        Ok(((page - 1) * page_num, page_num))
    }
}

/// Standard paginated response body.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, page_num: i64) -> Self {
        Self {
            items,
            total,
            page,
            page_num,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults() {
        let p = PaginationParams::default();
        let (page, page_num) = p.resolve().unwrap();
        assert_eq!(page, 1);
        assert_eq!(page_num, DEFAULT_PAGE_NUM);
        let (offset, limit) = p.offset_limit().unwrap();
        assert_eq!(offset, 0);
        assert_eq!(limit, DEFAULT_PAGE_NUM);
    }

    #[test]
    fn resolves_explicit() {
        let p: PaginationParams = serde_json::from_value(serde_json::json!({"page": 3, "pageNum": 50})).unwrap();
        let (page, page_num) = p.resolve().unwrap();
        assert_eq!(page, 3);
        assert_eq!(page_num, 50);
        let (offset, limit) = p.offset_limit().unwrap();
        assert_eq!(offset, 100);
        assert_eq!(limit, 50);
    }

    #[test]
    fn rejects_oversized_page_num() {
        let p: PaginationParams = serde_json::from_value(serde_json::json!({"pageNum": 101})).unwrap();
        assert!(p.resolve().is_err());
    }

    #[test]
    fn response_shape() {
        let page = Page::new(vec![1, 2], 2, 1, 20);
        let json = serde_json::to_value(&page).unwrap();
        assert_eq!(json["items"].as_array().unwrap().len(), 2);
        assert_eq!(json["total"], 2);
        assert_eq!(json["page"], 1);
        assert_eq!(json["pageNum"], 20);
    }
}
