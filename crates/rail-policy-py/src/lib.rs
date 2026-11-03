//! The Python binding of `rail-policy`.

use pyo3::prelude::*;

#[pymodule]
mod _native {
    use pyo3::prelude::*;

    /// Why the bundle can't be used, or `None` when it can.
    #[pyfunction]
    fn validate_bundle(json: &str) -> Option<String> {
        rail_policy::parse_bundle(json)
            .err()
            .map(|unusable| unusable.to_string())
    }
}
