//! The SPL exception classes (php-src `ext/spl/spl_exceptions.c`): the
//! `LogicException` and `RuntimeException` trees, all plain subclasses of
//! `Exception` with no members of their own.

use rphp_runtime::Registry;

/// `(class, parent)` in registration order (parents first).
const CLASSES: &[(&str, &str)] = &[
    ("LogicException", "Exception"),
    ("BadFunctionCallException", "LogicException"),
    ("BadMethodCallException", "BadFunctionCallException"),
    ("DomainException", "LogicException"),
    ("InvalidArgumentException", "LogicException"),
    ("LengthException", "LogicException"),
    ("OutOfRangeException", "LogicException"),
    ("RuntimeException", "Exception"),
    ("OutOfBoundsException", "RuntimeException"),
    ("OverflowException", "RuntimeException"),
    ("RangeException", "RuntimeException"),
    ("UnderflowException", "RuntimeException"),
    ("UnexpectedValueException", "RuntimeException"),
];

/// Register the SPL exception tree (after `zend_exceptions`).
pub(crate) fn register_classes(r: &mut Registry) {
    for (name, parent) in CLASSES {
        r.class(name).extends(parent).finish();
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::interp;

    #[test]
    fn spl_tree_hangs_off_exception() {
        let it = interp();
        let bmc = it.class_by_name(b"BadMethodCallException").unwrap();
        let logic = it.class_by_name(b"LogicException").unwrap();
        let runtime = it.class_by_name(b"RuntimeException").unwrap();
        let ex = it.class_by_name(b"Exception").unwrap();
        assert!(it.instanceof_class(bmc, logic));
        assert!(it.instanceof_class(bmc, ex));
        assert!(!it.instanceof_class(bmc, runtime));
        assert!(it.class(bmc).method(b"getMessage").is_some());
    }
}
