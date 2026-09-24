<?php
// mb_regex_set_options: the option string round trip (i x p m s l n and
// the syntax letters), how s/m change ^ $ and ., and the other syntaxes
// (Perl, Java, GNU, POSIX, grep, emacs) on their common constructs.
function t($p, $s) {
    $r = mb_ereg($p, $s, $m);
    echo json_encode($p), " ", json_encode($s), " => ", var_export($r, true), " ", json_encode($m), "\n";
}
echo mb_regex_set_options(), "\n";
echo mb_regex_set_options(''), "|", mb_regex_set_options(), "\n";
t("^b", "a\nb");
t("a.b", "a\nb");
echo mb_regex_set_options('m'), "\n";
t("^b", "a\nb");
t("a.b", "a\nb");
echo mb_regex_set_options('s'), "\n";
t("^b", "a\nb");
t("a$", "a\nb");
t("a.b", "a\nb");
echo mb_regex_set_options('imxslnjugcrzbd'), "\n";
foreach (['j', 'u', 'g', 'c', 'r', 'z', 'b', 'd', 'ip', 'ixmsln', 'sm', 'pr'] as $o) {
    echo $o, "=", mb_regex_set_options($o), "|", mb_regex_set_options(), "\n";
}
foreach (['q', 'e'] as $o) {
    try {
        mb_regex_set_options($o);
    } catch (\ValueError $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
echo mb_regex_set_options(), "\n";

echo "-- syntaxes\n";
$pats = [
    ["\\h", "f"], ["a+", "aa"], ["a\\+", "a+"], ["a?", "a"], ["a\\?", "a?"], ["a|b", "b"], ["a\\|b", "b"], ["(a)", "a"],
    ["\\(a\\)", "a"], ["\\(a\\)", "(a)"], ["a{2}", "aa"], ["a\\{2\\}", "aa"], ["(?i)A", "a"], ["(?s).", "\n"], ["(?m).", "\n"],
    ["(?m)^b", "a\nb"], [".", "\n"], ["^b", "a\nb"], ["a$", "a\nb"], ["\\Qa.\\E", "a."], ["(?<n>a)(b)", "ab"], ["\\w", "é"],
    ["\\d", "٣"], ["[[:alpha:]]", "é"], ["\\p{L}", "é"], ["a{,2}", "aa"], ["a{1,2}+", "aaaa"], ["\\<a\\>", "a"], ["\\u0041", "A"],
    ["a++", "aa"], ["[a&&b]", "b"], ["\\x{41}", "A"], ["\\A", "a"], ["\\v", "\x0b"], ["\\1(a)", "a"], ["(a)\\1", "aa"],
    ["*a", "*a"], ["a**", "aa"], ["a{2,1}", "aa"], ["(?~a)", "b"], ["\\K", "a"], ["\\R", "\n"], ["\\X", "a"],
];
foreach ($pats as [$p, $s]) {
    echo str_pad(json_encode($p), 16), str_pad(json_encode($s), 10);
    foreach (['r', 'z', 'j', 'u', 'g', 'b', 'd'] as $o) {
        set_error_handler(function ($no, $str) use (&$err) { $err = $str; return true; });
        $err = null;
        $r = mb_ereg_search_init($s, $p, $o) ? mb_ereg_search_regs() : 'I';
        restore_error_handler();
        echo " $o=", $err ? 'E:' . substr($err, strpos($err, 'err: ') + 5) : ($r === false ? 'F' : json_encode($r));
    }
    echo "\n";
}
