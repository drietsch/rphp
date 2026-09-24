<?php
// Oniguruma's compile errors ("mbregex compile err: …"), the invalid-
// pattern warning, and the ValueErrors of the regex functions.
function t($p, $s = "a") {
    $r = mb_ereg($p, $s, $m);
    echo json_encode($p), " => ", var_export($r, true), "\n";
}
foreach (["(", ")", "[", "[]", "[a", "*a", "a|*", "{1}", "a{100001}", "\\", "[b-a]", "[[:foo:]]",
          "[[:alpha]", "[\\]", "\\p{Foo}", "\\p{}", "\\p{L&}", "\\p{Hiragana", "(?<1a>x)", "(?<>x)",
          "(?<a", "\\k<zz>", "\\k'q'(?'q'a)", "(?", "(?i", "(?i:", "(?#abc", "(?q)a", "(?s).", "(?)a",
          "(?a)\\w", "\\xe3", "\\xe3\\x81", "\\x{41", "\\u12", "\\uzz", "[\\w-a]", "[a-\\w]", "[\\d-z]",
          "[[:alpha:]-z]", "[a-[:digit:]]", "^*", "\\b+", "(?=a)*a", "(?=a){2}a", "\\A?a", "\\8", "\\7",
          "(a)\\2", "(?<n>a)\\1", "(?<n>a)\\g<1>", "(?<n>a)\\k<1>", "\\g<2>(a)", "\\g<x>", "\\C-",
          "\\c", "\\M-"] as $p) {
    t($p);
}
echo "-- invalid pattern bytes\n";
var_dump(mb_ereg("\xff", "a"));
var_dump(mb_ereg_replace("(", "x", "a"));
var_dump(mb_split("(", "a"));
var_dump(mb_ereg_match("(?<x>a", "ab"));

echo "-- ValueErrors\n";
foreach ([
    fn() => mb_ereg('', 'a'),
    fn() => mb_eregi('', 'a'),
    fn() => mb_ereg_search_init('ab', ''),
    fn() => mb_ereg_replace('a', 'x', 'ab', 'e'),
    fn() => mb_ereg_replace('a', 'x', 'ab', 'q'),
    fn() => mb_ereg_match('a', 'ab', '?'),
    fn() => mb_regex_set_options('e'),
    fn() => mb_regex_encoding('latin1'),
    fn() => mb_regex_encoding('UTF-7'),
    fn() => mb_ereg([], 'a'),
    fn() => mb_ereg_replace_callback('a', 'no_such_function', 'a'),
] as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
echo "-- empty patterns that are fine\n";
var_dump(mb_ereg_replace('', '-', 'ab'), mb_split('', 'ab'), mb_ereg_match('', 'ab'));
