<?php
// string.c: trim family with `a..z` ranges (and their warnings), str_pad,
// str_word_count char lists, number_format incl. negative decimals, ucwords.

var_dump(trim("abcxyzcba", "a..c"), trim("  x  ", " "), rtrim("xaa", "a"), ltrim("0012", "0"), trim("\x00\x0b x \t\r\n"), trim("[x]", "[]"), trim("a..b", "."), ltrim("...a", "."), rtrim("a..", "."));
var_dump(trim("abc", "c..a"));       // warning: not incrementing
var_dump(trim("..", ".."));          // warning: nothing to the left
var_dump(rtrim("abc.", "c.."));      // warning: nothing to the right
var_dump(ltrim("xyz0123", "0..9x..z"), trim("\x01\x02abc\xfe\xff", "\x00..\x1f\x80..\xff"));
var_dump(str_pad("5", 3, "0", 0), str_pad("5", 3, "0", 1), str_pad("5", 3, "0", 2), str_pad("ab", 7, "-=", 2), str_pad("abc", 2), str_pad("x", 5, "ab", 0), str_pad("", 3, "y"));
var_dump(str_word_count("Hello world foo"), str_word_count("hello-world", 1), str_word_count("-hello- world-", 1), str_word_count("hello world", 2, "-"), str_word_count("hello wor1d", 1, "1"), str_word_count("a..z 0-9", 1, "0..9"), str_word_count("fred's don't x'", 1), str_word_count("a b", 1, null));
var_dump(str_word_count("a b", 1, "b..a"));   // warning: not incrementing
var_dump(number_format(1234.5678), number_format(1234.5678, 2), number_format(-1234.5678, 2), number_format(1234.5678, 2, ".", " "), number_format(0.005, 2), number_format(2.5, 0), number_format(-2.5, 0), number_format(99.995, 2));
var_dump(number_format(1234.5678, -2), number_format(1234.5678, -1), number_format(-1250, -2), number_format(1250, -2), number_format(1234.5, -5), number_format(0.5), number_format(1.5), number_format(-0.4), number_format(-0.5), number_format(1e15, 2), number_format(1e20), number_format("1234.5"), number_format(1234.5678, 2, "", ""), number_format(1234.5678, 2, "DOT", "TS"), number_format(1e-10, 2), number_format(-1e-10, 2), number_format(0.125, 2), number_format(0.135, 2), number_format(1.005, 2), number_format(2.675, 2), number_format(123456789.123456789, 5), number_format(-0.0), number_format(-1234567.891, 1, ",", "."));
var_dump(ucwords("hello_world-foo bar", "_-"), ucwords("hello world"), ucwords("a\tb\nc\rd\x0be\x0cf"), lcfirst("ABC"), ucfirst("élan"), ucfirst(""), strrev(""), strpbrk("Hello World", "oW"), str_repeat("ab", 3), str_repeat("x", 0));
var_dump(chr(-1) === "\xff", chr(256) === "\x00", ord("\xff"), ord(""), strtolower("ÀBC"), strtoupper("àbc"), nl2br("a\r\n\nb\n\rc\rd", false));
