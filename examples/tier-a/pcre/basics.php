<?php
// Differential snippet for the `pcre` extension. Exercises every implemented
// function so its output can be diffed against stock PHP 8.5.

// ---- preg_quote ----
echo preg_quote("a.b*c"), "\n";
echo preg_quote(". + * ? [ ^ ] = ! < > | : - #"), "\n";
echo preg_quote("http://a.b/c", "/"), "\n";
echo preg_quote("a@b@c", "@"), "\n";

// ---- preg_match ----
var_dump(preg_match("/foo/", "a foo b"));
var_dump(preg_match("/foo/", "a bar b"));
var_dump(preg_match("/FOO/i", "a foo b"));
var_dump(preg_match("/^abc$/m", "x\nabc\ny"));
var_dump(preg_match("/^abc$/", "x\nabc\ny"));
var_dump(preg_match("/a.b/s", "a\nb"));
var_dump(preg_match("/a.b/", "a\nb"));
var_dump(preg_match("(a.c)", "axc"));
var_dump(preg_match("{a.c}", "axc"));

// ---- preg_replace ----
echo preg_replace("/a/", "X", "banana"), "\n";
echo preg_replace("/(\w)(\w)/", '$2$1', "ab cd"), "\n";
echo preg_replace("/(\d+)/", '[\1]', "a12b34"), "\n";
echo preg_replace("/\d+/", '<$0>', "a12b34"), "\n";
echo preg_replace("/(\d)(\d)/", '${2}${1}', "12 34"), "\n";
echo preg_replace("//", "-", "abc"), "\n";
echo preg_replace("/z/", "X", "abc"), "\n";

// ---- preg_split ----
print_r(preg_split("/,/", "a,b,c"));
print_r(preg_split("/\s+/", "a  b   c"));
print_r(preg_split("/,/", "a,b,c,d", 2));
print_r(preg_split("/,/", "a,,b,", -1, 1));
print_r(preg_split("//", "abc"));
print_r(preg_split("//", "abc", -1, 1));

// ---- preg_grep ----
print_r(preg_grep("/^a/", array("apple", "banana", "avocado", "cherry")));
print_r(preg_grep("/[0-9]/", array(10 => "foo1", 20 => "bar", 30 => "baz2")));
print_r(preg_grep("/^a/", array("apple", "banana", "avocado"), 1));
