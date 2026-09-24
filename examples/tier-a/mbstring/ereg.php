<?php
// mb_ereg / mb_eregi over Oniguruma's Ruby syntax: anchors under the
// default "pr" options, \h, named groups (plain groups stop capturing, the
// name order of Oniguruma's hash table), empty groups as false, nested
// repeats, class intersections, properties, escapes and raw bytes.
function t($p, $s) {
    $r = mb_ereg($p, $s, $m);
    echo json_encode($p), " ", json_encode($s), " => ", var_export($r, true), " ", json_encode($m), "\n";
}
echo "-- anchors and dot\n";
t("^b", "a\nb");
t("a$", "a\nb");
t("a$", "a\n");
t("a$", "a\n\n");
t("a.b", "a\nb");
t("\\N", "\n");
t("\\Aab\\z", "ab");
t("a\\Z", "a\n");

echo "-- Ruby escapes\n";
t("\\h+", "xx1f");
t("\\H+", "1fxx");
t("\\u3042", "あ");
t("\\x{3042}", "あ");
t("\\xe3\\x81\\x82+", "あああ");
t("\\343\\201\\202", "あ");
t("\\o{101}", "A");
t("\\x41\\x4", "A\x04");
t("\\xg", "\0g");
t("\\x", "x");
t("\\v", "\x0b");
t("\\t\\n\\r\\f\\e\\a", "\t\n\r\f\x1b\x07");
t("\\cA\\C-b", "\x01\x02");
t("\\M-a", "á");
t("\\Qa.\\E", "Qa.E");
t("\\q\\j\\O", "qjO");
t("\\\\\\/\\-\\#\\ \\@", "\\/-# @");
t("\\K", "a");
t("a\\Kb", "ab");
t("\\R", "\r\n");
t("\\X", "e\u{301}");

echo "-- groups and names\n";
t("(?<x>a)(b)", "ab");
t("(?<x>a)(?<y>b)?(?<z>c)", "ac");
t("(?<z>a)(?<a>b)(?<m>c)", "abc");
t("(?<a>.)(?<b>.)(?<c>.)(?<d>.)(?<e>.)(?<f>.)(?<g>.)(?<h>.)(?<i>.)(?<j>.)(?<k>.)(?<l>.)", "abcdefghijkl");
t("(?<foo>.)(?<bar>.)(?<baz>.)(?<qux>.)(?<name>.)(?<group>.)", "abcdef");
t("(?<名前>a)(?<a-b>b)(?'q'c)", "abc");
t("(?<a>x)(?<a>y)", "xy");
t("(?<a>x)|(?<a>y)", "y");
t("\\w+((?<punct>？)|(?<punct>！))", "中！");
t("(?<a>x)\\k<a>", "xx");
t("(?<n>a)\\k<n+0>", "aa");
t("(?<n>a|b\\g<n>)", "bba");
t("(a)()(b)?", "a");
t("()", "b");
t("(|a)", "a");
t("(a)\\k<-1>", "aa");
t("\\g<1>(a)", "aa");
t("(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)\\10", "abcdefghijj");
t("(a)\\10", "a\x08");
t("(a)\\18", "a\x018");
t("(a)?(?(1)b|c)", "ab");
t("(?<n>a)?(?(<n>)b|c)", "c");
t("(?i:A)b", "aB");
t("(?i-i:a)", "A");
t("a(?i)b|c", "c");
t("a(?i)b|c", "aC");
t("(?m).", "\n");
t("(?x) a b # c\n c", "abc");
t("(?x)a b {2}", "abb");
t("(?x)a\\ b[ ]c", "a b c");
t("(?>a+)a", "aaa");
t("(?<=ab|c)d", "abd");
t("(?<!a)b", "b");
t("(?~abc)", "xabcy");
t("(?~abc)", "abab");

echo "-- repeats\n";
t("a{,2}", "aaa");
t("a{2}?", "aaa");
t("a{1,2}+", "aaaa");
t("a{3,1}", "aaaa");
t("a{3,1}b", "aab");
t("a**", "aaa");
t("x{2}{3}", "xxxxxx");
t("a{", "a{");
t("a{1,x}", "a{1,x}");
t("a{,}", "a{,}");
t("a{0}", "aaa");
t("a{1,}?", "aaa");
t("a?+", "a");
t("a??", "a");
t("a{2,3}?", "aaa");
t("a|", "b");

echo "-- classes\n";
t("[a-z&&[^aeiou]]+", "abc");
t("[\\w&&\\d]+", "a12");
t("[a-z&&b-y&&c]+", "abc");
t("[^a-z&&b]", "b");
t("[&&a]", "a");
t("[a&]", "&");
t("[a[b]]", "b");
t("[^a[b]]", "b");
t("[]a]", "]");
t("[^]a]", "b");
t("[a-]", "-");
t("[a-z-0]", "-");
t("[--a]", "-");
t("[\\-]", "-");
t("[[:alpha:][:digit:]]+", "é1");
t("[[:^alpha:]]", "1");
t("[[:word:]]+", "a_1");
t("[[:xdigit:]]+", "fa9５");
t("[\\x{3042}-\\x{3044}]+", "あいう");
t("[\\xe3\\x81\\x82]", "あ");
t("[\\xff]", "a");
t("(?i)[a-z]", "Z");
t("(?i)[^a]", "A");

echo "-- properties\n";
t("\\p{Alnum}+", "ab1-");
t("\\p{Word}+", "ab1-");
t("\\p{Hiragana}+", "あいカ");
t("\\p{^Alpha}+", "12a");
t("\\P{L}", "1");
t("\\p{In_Hiragana}+", "あ");
t("\\p{Hira gana}\\p{hira-gana}", "ああ");
t("[\\p{Hiragana}a]+", "あa");
t("[^\\p{Hiragana}]", "あa");
t("\\p{Greek}\\p{Han}\\p{Latin}", "α漢a");
t("\\pL", "pL");
t("\\w+", "éa1");
t("\\d", "٣");
t("ß", "SS");
t("(?i)ß", "ss");

echo "-- mb_eregi\n";
var_dump(mb_eregi("É", "é"), mb_eregi("straße", "STRASSE"), mb_eregi("[A-Z]+", "abc", $m), $m);

echo "-- no match, invalid subjects\n";
$m = 5;
var_dump(mb_ereg("z", "xa", $m), $m);
var_dump(mb_ereg("a*", "b", $m), $m);
var_dump(mb_ereg("a", "\xff", $m), $m);
var_dump(mb_ereg("a", "xa"));
var_dump(mb_ereg(1, "1"));
