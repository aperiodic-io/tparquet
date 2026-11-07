//! DuckDB vectors of the C API v2 as [`VectorView`]s, for [`crate::chunk::convert`]: the same
//! conversion to Arrow, with the same type rules, as the C API v1 build.
//!
//! A [`Flat`] is taken from a vector once, before the conversion reads it: each vector of the
//! tree is made flat (`duckdb_v2_vector_flatten`, a no-op for the flat vectors a column data
//! collection scan yields), and its data, validity and children are read through
//! `duckdb_v2_vector_get_view` and `duckdb_v2_vector_get_child`. The children are checked
//! against the column's type, so that the conversion never asks for a child the vector lacks.

use crate::chunk::VectorView;
use crate::types::SqlType;
use libduckdb_sys::v2 as s;
use std::sync::Arc;

/// A flat vector and its children, read through the C API v2. The pointers are borrowed from
/// the chunk the vector belongs to: a `Flat` lives no longer than that chunk's scan.
#[derive(Debug, Clone)]
pub struct Flat {
    data: *const u8,
    validity: *const u64,
    size: usize,
    children: Arc<[Flat]>,
}

/// The types of the child vectors of a vector of type `t` (a MAP's child is the STRUCT of its
/// keys and values).
pub fn children_of(t: &SqlType) -> Vec<SqlType> {
    match t {
        SqlType::List(c) | SqlType::Array(c, _) => vec![(**c).clone()],
        SqlType::Map(k, v) => {
            vec![SqlType::Struct(vec![("key".into(), (**k).clone()), ("value".into(), (**v).clone())])]
        }
        SqlType::Struct(fields) => fields.iter().map(|(_, t)| t.clone()).collect(),
        _ => vec![],
    }
}

impl Flat {
    /// Reads the vector `v` of type `t` (flattened in place first).
    pub fn of(v: s::duckdb_v2_vector_handle, t: &SqlType) -> Result<Self, String> {
        call!(duckdb_v2_vector_flatten(v))?;
        let mut view = s::duckdb_v2_vector_view {
            data: std::ptr::null(),
            validity: std::ptr::null(),
            sel: std::ptr::null(),
            count: 0,
        };
        call!(duckdb_v2_vector_get_view(v, &mut view))?;
        let mut size = 0u64;
        call!(duckdb_v2_vector_get_size(v, &mut size))?;
        let types = children_of(t);
        let mut n = 0u64;
        call!(duckdb_v2_vector_get_child_count(v, &mut n))?;
        if n != types.len() as u64 {
            return Err(format!("a vector of type {t} has {n} children, not {}", types.len()));
        }
        let children = types
            .iter()
            .enumerate()
            .map(|(i, ct)| {
                let mut c: s::duckdb_v2_vector_handle = std::ptr::null_mut();
                call!(duckdb_v2_vector_get_child(v, i as u64, &mut c))?;
                Flat::of(c, ct)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Flat { data: view.data.cast(), validity: view.validity, size: size as usize, children: children.into() })
    }
}

impl VectorView for Flat {
    fn data(&self) -> *const u8 {
        self.data
    }
    fn validity(&self) -> *const u64 {
        self.validity
    }
    // the children were checked against the type in `Flat::of`, and `convert` asks only for the
    // children of the type it converts
    fn list_child(&self) -> (Self, usize) {
        (self.children[0].clone(), self.children[0].size)
    }
    fn struct_child(&self, i: usize) -> Self {
        self.children[i].clone()
    }
    fn array_child(&self) -> Self {
        self.children[0].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_types_have_one_child_vector_per_child_type() {
        let int = || Box::new(SqlType::Integer);
        assert_eq!(children_of(&SqlType::List(int())), vec![SqlType::Integer]);
        assert_eq!(children_of(&SqlType::Array(int(), 3)), vec![SqlType::Integer]);
        assert_eq!(
            children_of(&SqlType::Map(Box::new(SqlType::Varchar), int())),
            vec![SqlType::Struct(vec![("key".into(), SqlType::Varchar), ("value".into(), SqlType::Integer)])]
        );
        let fields = vec![("a".to_string(), SqlType::Double), ("b".to_string(), SqlType::List(int()))];
        assert_eq!(children_of(&SqlType::Struct(fields)), vec![SqlType::Double, SqlType::List(int())]);
        assert!(children_of(&SqlType::Varchar).is_empty());
        assert!(children_of(&SqlType::Enum(vec!["x".into()])).is_empty());
    }
}
