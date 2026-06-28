//! End-to-end tests driving the full lexer -> parser -> compiler -> runtime
//! pipeline through the CLI SAPI.

use std::path::PathBuf;

use rphp_sapi_cli::{eval_to_bytes, eval_to_string, run};

/// Evaluate source, asserting it succeeds, and return its stdout.
fn eval_ok(src: &[u8]) -> String {
    eval_to_string(src).unwrap_or_else(|e| panic!("expected success, got error:\n{e}"))
}

#[test]
fn arithmetic_precedence() {
    assert_eq!(eval_ok(b"<?php echo 1 + 2 * 3;"), "7");
}

#[test]
fn pow_is_right_associative() {
    // 2 ** (3 ** 2) == 2 ** 9 == 512
    assert_eq!(eval_ok(b"<?php echo 2 ** 3 ** 2;"), "512");
}

#[test]
fn variables_and_assignment() {
    assert_eq!(eval_ok(b"<?php $x = 5; $y = 10; echo $x + $y;"), "15");
}

#[test]
fn if_else_branch() {
    // 3 <=> 2 == 1 (truthy) => the then-branch runs.
    assert_eq!(eval_ok(b"<?php if (3 <=> 2) echo 1; else echo 2;"), "1");
}

#[test]
fn while_loop_sums_one_to_five() {
    assert_eq!(
        eval_ok(b"<?php $i = 1; $s = 0; while ($i <= 5) { $s = $s + $i; $i = $i + 1; } echo $s;"),
        "15"
    );
}

#[test]
fn function_definition_and_call() {
    assert_eq!(
        eval_ok(b"<?php function add($a, $b) { return $a + $b; } echo add(3, 4);"),
        "7"
    );
}

#[test]
fn recursion_factorial() {
    assert_eq!(
        eval_ok(b"<?php function f($n) { if ($n <= 1) return 1; return $n * f($n - 1); } echo f(5);"),
        "120"
    );
}

// ---- strings ---------------------------------------------------------------

#[test]
fn string_concatenation() {
    assert_eq!(eval_ok(br#"<?php echo "Hello, " . "world!";"#), "Hello, world!");
}

#[test]
fn concat_below_addition_php8() {
    // PHP 8 precedence: "x" . 1 + 2  ==  "x" . (1 + 2)  ==  "x3"
    assert_eq!(eval_ok(br#"<?php echo "x" . 1 + 2;"#), "x3");
}

#[test]
fn numeric_string_arithmetic() {
    assert_eq!(eval_ok(br#"<?php echo "10" + 5;"#), "15");
    assert_eq!(eval_ok(br#"<?php echo "3" . "4" * 2;"#), "38"); // 3 . (4*2)
}

#[test]
fn double_quote_escapes() {
    assert_eq!(eval_ok(br#"<?php echo "a\tb\nc";"#), "a\tb\nc");
}

#[test]
fn single_quotes_are_literal() {
    // \n stays a backslash-n in single quotes.
    assert_eq!(eval_ok(br"<?php echo 'a\nb';"), r"a\nb");
}

#[test]
fn simple_variable_interpolation() {
    assert_eq!(
        eval_ok(br#"<?php $name = "PHP"; echo "Hi $name!";"#),
        "Hi PHP!"
    );
}

#[test]
fn brace_variable_interpolation() {
    assert_eq!(eval_ok(br#"<?php $x = 7; echo "v={$x}.";"#), "v=7.");
}

#[test]
fn interpolation_stringifies_numbers() {
    // A lone interpolated value is string-cast, and concatenation joins parts.
    assert_eq!(eval_ok(br#"<?php $n = 42; echo "$n";"#), "42");
}

#[test]
fn string_comparison_is_lexical() {
    assert_eq!(eval_ok(br#"<?php if ("abc" < "abd") echo "yes"; else echo "no";"#), "yes");
}

#[test]
fn echo_is_binary_safe() {
    // A byte that is not valid UTF-8 must round-trip through echo unchanged.
    let out = eval_to_bytes(b"<?php echo \"\\xff\\x00A\";").unwrap();
    assert_eq!(out, vec![0xff, 0x00, b'A']);
}

// ---- arrays -----------------------------------------------------------------

#[test]
fn array_literal_and_index() {
    assert_eq!(eval_ok(b"<?php $a = [10, 20, 30]; echo $a[1];"), "20");
    assert_eq!(eval_ok(b"<?php echo array(7, 8)[1];"), "8");
}

#[test]
fn string_keys() {
    assert_eq!(eval_ok(br#"<?php $a = ["x" => 1, "y" => 2]; echo $a["y"];"#), "2");
}

#[test]
fn append_and_overwrite() {
    assert_eq!(eval_ok(b"<?php $a = []; $a[] = 5; $a[] = 6; echo $a[0] . $a[1];"), "56");
    assert_eq!(eval_ok(b"<?php $a = [1, 2]; $a[0] = 9; echo $a[0];"), "9");
}

#[test]
fn append_autovivifies_array() {
    assert_eq!(eval_ok(b"<?php $a[] = 7; echo $a[0];"), "7");
    assert_eq!(eval_ok(b"<?php $a[3] = 'x'; echo $a[3];"), "x");
}

#[test]
fn int_and_string_keys_unify() {
    // "5" and 5 are the same key; "05" stays distinct.
    assert_eq!(eval_ok(br#"<?php $a = []; $a["5"] = "i"; echo $a[5];"#), "i");
    assert_eq!(eval_ok(br#"<?php $a = []; $a["05"] = "s"; echo $a["05"];"#), "s");
}

#[test]
fn next_key_follows_highest_int() {
    assert_eq!(eval_ok(b"<?php $a = [5 => 'a']; $a[] = 'b'; echo $a[6];"), "b");
}

#[test]
fn nested_array_read() {
    assert_eq!(eval_ok(b"<?php $a = [[1, 2], [3, 4]]; echo $a[1][0];"), "3");
}

#[test]
fn echo_array_is_the_word_array() {
    assert_eq!(eval_ok(b"<?php echo [1, 2];"), "Array");
}

#[test]
fn array_union_operator() {
    assert_eq!(eval_ok(b"<?php $c = [1, 2] + [9, 9, 9]; echo $c[0] . $c[2];"), "19");
}

#[test]
fn copy_on_write_value_semantics() {
    // $b is a copy; appending to it must not extend $a. $a[1] is absent -> "".
    assert_eq!(
        eval_ok(b"<?php $a = [1]; $b = $a; $b[] = 2; echo $b[0] . $b[1] . '|' . $a[0] . '[' . $a[1] . ']';"),
        "12|1[]"
    );
}

#[test]
fn foreach_value() {
    assert_eq!(
        eval_ok(b"<?php $a = [1, 2, 3]; $s = 0; foreach ($a as $v) { $s = $s + $v; } echo $s;"),
        "6"
    );
}

#[test]
fn foreach_key_value() {
    assert_eq!(
        eval_ok(br#"<?php $a = ["a" => 1, "b" => 2]; foreach ($a as $k => $v) { echo $k . "=" . $v . ";"; }"#),
        "a=1;b=2;"
    );
}

#[test]
fn foreach_over_literal_and_snapshot() {
    assert_eq!(eval_ok(b"<?php foreach ([1, 2, 3] as $v) echo $v;"), "123");
    // Mutating the source array inside the loop does not affect iteration.
    assert_eq!(
        eval_ok(b"<?php $a = [1, 2]; foreach ($a as $v) { $a[] = 9; echo $v; }"),
        "12"
    );
}

#[test]
fn string_offset_read() {
    assert_eq!(eval_ok(br#"<?php $s = "hello"; echo $s[1] . $s[-1];"#), "eo");
}

#[test]
fn nested_write_is_a_clean_error() {
    let err = eval_to_string(b"<?php $a = []; $a[0][1] = 5;").unwrap_err();
    assert!(err.contains("nested array assignment"), "got: {err}");
}

#[test]
fn division_by_zero_is_an_error() {
    let err = eval_to_string(b"<?php echo 1 / 0;").unwrap_err();
    assert!(
        err.contains("Division by zero"),
        "expected a division-by-zero error, got: {err}"
    );
}

#[test]
fn syntax_error_reports_diagnostic() {
    // A missing right-hand side is a parse error; it must not panic and must
    // surface a rendered diagnostic rather than running.
    let err = eval_to_string(b"<?php $x = ;").unwrap_err();
    assert!(!err.is_empty(), "expected a rendered diagnostic");
}

#[test]
fn undefined_function_reports_diagnostic() {
    let err = eval_to_string(b"<?php echo nope();").unwrap_err();
    assert!(
        err.contains("undefined function"),
        "expected an undefined-function diagnostic, got: {err}"
    );
}

// ---- stdlib builtins (resolved through rphp-stdlib) -------------------------

#[test]
fn string_builtins() {
    assert_eq!(eval_ok(br#"<?php echo strlen("hello");"#), "5");
    assert_eq!(eval_ok(br#"<?php echo strtoupper("aB") . strtolower("aB");"#), "ABab");
    assert_eq!(eval_ok(br#"<?php echo ucfirst("php");"#), "Php");
    assert_eq!(eval_ok(br#"<?php echo str_repeat("ab", 3);"#), "ababab");
    assert_eq!(eval_ok(br#"<?php echo substr("abcdef", 1, 3);"#), "bcd");
    assert_eq!(eval_ok(br#"<?php echo substr("abcdef", -2);"#), "ef");
    assert_eq!(eval_ok(br#"<?php echo strpos("hello", "ll");"#), "2");
    assert_eq!(eval_ok(br#"<?php echo str_replace("a", "X", "banana");"#), "bXnXnX");
    assert_eq!(eval_ok(br#"<?php echo "[" . trim("  hi  ") . "]";"#), "[hi]");
    assert_eq!(eval_ok(br#"<?php echo implode("-", ["a", "b", "c"]);"#), "a-b-c");
    assert_eq!(eval_ok(br#"<?php $p = explode(",", "x,y,z"); echo $p[2];"#), "z");
    assert_eq!(eval_ok(br#"<?php echo ord("A") . chr(98);"#), "65b");
}

#[test]
fn str_predicates_return_real_bools() {
    // A bool result echoes as "1"/"" — the byte-exact PHP form.
    assert_eq!(eval_ok(br#"<?php echo str_contains("abc", "b");"#), "1");
    assert_eq!(eval_ok(br#"<?php echo str_starts_with("abc", "x");"#), "");
    assert_eq!(eval_ok(br#"<?php echo str_ends_with("abc", "bc");"#), "1");
}

#[test]
fn array_builtins() {
    assert_eq!(eval_ok(b"<?php echo count([1, 2, 3]);"), "3");
    assert_eq!(eval_ok(b"<?php echo array_sum([1, 2, 3, 4]);"), "10");
    assert_eq!(eval_ok(b"<?php echo in_array(2, [1, 2, 3]);"), "1");
    assert_eq!(eval_ok(b"<?php echo in_array(9, [1, 2, 3]);"), "");
    assert_eq!(eval_ok(br#"<?php echo array_key_exists("k", ["k" => 1]);"#), "1");
    assert_eq!(eval_ok(b"<?php $m = array_merge([1], [2, 3]); echo $m[2];"), "3");
    assert_eq!(eval_ok(b"<?php $r = array_reverse([1, 2, 3]); echo $r[0];"), "3");
    assert_eq!(eval_ok(b"<?php echo implode(\",\", range(1, 4));"), "1,2,3,4");
    assert_eq!(eval_ok(b"<?php echo implode(\"\", range('a', 'c'));"), "abc");
}

#[test]
fn math_builtins() {
    assert_eq!(eval_ok(b"<?php echo abs(-5);"), "5");
    assert_eq!(eval_ok(b"<?php echo max(3, 7, 2);"), "7");
    assert_eq!(eval_ok(b"<?php echo min([4, 1, 8]);"), "1");
    assert_eq!(eval_ok(b"<?php echo floor(3.9) . ceil(3.1);"), "34");
    assert_eq!(eval_ok(b"<?php echo round(3.14159, 2);"), "3.14");
    assert_eq!(eval_ok(b"<?php echo sqrt(81);"), "9");
    assert_eq!(eval_ok(b"<?php echo intdiv(17, 5);"), "3");
}

#[test]
fn function_aliases_resolve() {
    // sizeof => count, join => implode are registry aliases.
    assert_eq!(eval_ok(b"<?php echo sizeof([1, 2, 3]);"), "3");
    assert_eq!(eval_ok(br#"<?php echo join("/", ["a", "b"]);"#), "a/b");
}

#[test]
fn case_insensitive_builtin_names() {
    // PHP function names are case-insensitive.
    assert_eq!(eval_ok(br#"<?php echo STRLEN("abcd");"#), "4");
    assert_eq!(eval_ok(br#"<?php echo StrToUpper("hi");"#), "HI");
}

#[test]
fn intdiv_by_zero_is_a_runtime_error() {
    let err = eval_to_string(b"<?php echo intdiv(1, 0);").unwrap_err();
    assert!(err.contains("Division by zero"), "got: {err}");
}

#[test]
fn wrong_builtin_arg_count_is_a_compile_error() {
    // strlen takes exactly one argument.
    let err = eval_to_string(br#"<?php echo strlen("a", "b");"#).unwrap_err();
    assert!(err.contains("exactly 1"), "got: {err}");
}

#[test]
fn var_dump_matches_php() {
    assert_eq!(eval_ok(b"<?php var_dump(42);"), "int(42)\n");
    assert_eq!(eval_ok(b"<?php var_dump(true, null);"), "bool(true)\nNULL\n");
    assert_eq!(eval_ok(br#"<?php var_dump("hi");"#), "string(2) \"hi\"\n");
    assert_eq!(
        eval_ok(b"<?php var_dump([1, 2]);"),
        "array(2) {\n  [0]=>\n  int(1)\n  [1]=>\n  int(2)\n}\n"
    );
}

#[test]
fn print_r_return_mode() {
    assert_eq!(
        eval_ok(br#"<?php echo print_r([1, 2], true);"#),
        "Array\n(\n    [0] => 1\n    [1] => 2\n)\n"
    );
}

#[test]
fn builtin_const_args_run_through_interpreter() {
    // is_numeric on a numeric vs non-numeric string; gettype + based intval.
    assert_eq!(eval_ok(br#"<?php echo is_numeric("1.5e3");"#), "1");
    assert_eq!(eval_ok(br#"<?php echo is_numeric("12abc");"#), "");
    assert_eq!(eval_ok(br#"<?php echo gettype(3.5);"#), "double");
    assert_eq!(eval_ok(br#"<?php echo intval("0x1A", 16);"#), "26");
}

// ---- argument-handling tests through the public `run` entry point ----------

fn write_temp(name: &str, contents: &[u8]) -> PathBuf {
    let mut path = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("rphp_test_{}_{}_{}.php", std::process::id(), nanos, name));
    std::fs::write(&path, contents).expect("write temp file");
    path
}

#[test]
fn run_subcommand_executes_file() {
    let path = write_temp("run", b"<?php echo 6 * 7;");
    let code = run(vec!["run".to_string(), path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn bare_file_argument_executes() {
    let path = write_temp("bare", b"<?php echo 1;");
    let code = run(vec![path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
}

#[test]
fn help_flag_exits_zero() {
    assert_eq!(run(vec!["--help".to_string()]), 0);
    assert_eq!(run(vec!["-h".to_string()]), 0);
}

#[test]
fn no_args_exits_zero() {
    assert_eq!(run(vec![]), 0);
}

#[test]
fn unknown_flag_exits_two() {
    assert_eq!(run(vec!["--nope".to_string()]), 2);
}

#[test]
fn missing_file_exits_one() {
    let code = run(vec!["/this/path/does/not/exist_rphp.php".to_string()]);
    assert_eq!(code, 1);
}

#[test]
fn runtime_fault_exits_255() {
    let path = write_temp("divzero", b"<?php echo 1 / 0;");
    let code = run(vec![path.to_string_lossy().into_owned()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 255);
}
