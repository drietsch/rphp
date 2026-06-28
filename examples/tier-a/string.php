<?php
// Differential snippet for the string extension additions.
// Every implemented function is exercised; output is diffed against stock PHP.

// --- strrev ---
var_dump(strrev("Hello"));
var_dump(strrev(""));

// --- ucwords ---
var_dump(ucwords("hello world foo"));
var_dump(ucwords("hello-world|baz", "-|"));

// --- str_pad (0=LEFT, 1=RIGHT, 2=BOTH) ---
var_dump(str_pad("5", 3, "0", 0));
var_dump(str_pad("5", 3, "0", 1));
var_dump(str_pad("5", 3, "0", 2));
var_dump(str_pad("ab", 7, "-=", 2));
var_dump(str_pad("abc", 2));

// --- str_split ---
var_dump(str_split("hello"));
var_dump(str_split("abcdefg", 3));
var_dump(str_split(""));

// --- substr_count ---
var_dump(substr_count("hello world", "o"));
var_dump(substr_count("ababab", "ab"));
var_dump(substr_count("hellohello", "l", 1, 4));

// --- strrpos / stripos / strripos ---
var_dump(strrpos("hello world o", "o"));
var_dump(strrpos("hello world", "o", 5));
var_dump(strrpos("hello world", "o", -3));
var_dump(strrpos("hello", "x"));
var_dump(stripos("Hello World", "world"));
var_dump(stripos("abc", "x"));
var_dump(strripos("Hello World Hello", "hello"));

// --- strstr / stristr / strrchr / strpbrk ---
var_dump(strstr("user@example.com", "@"));
var_dump(strstr("user@example.com", "@", true));
var_dump(strstr("Hello", "xyz"));
var_dump(stristr("HELLO world", "WORLD"));
var_dump(strrchr("a/b/c/d", "/"));
var_dump(strrchr("hello", "x"));
var_dump(strpbrk("Hello World", "oW"));
var_dump(strpbrk("test", "xyz"));

// --- comparisons ---
var_dump(strcmp("a", "c"));
var_dump(strcmp("c", "a"));
var_dump(strcmp("abc", "abc"));
var_dump(strcmp("ab", "abc"));
var_dump(strcasecmp("ABC", "abc"));
var_dump(strcasecmp("a", "C"));
var_dump(strncmp("Hello", "Help", 3));
var_dump(strncmp("Hello", "Help", 4));
var_dump(strncasecmp("HELLO", "hello world", 5));

// --- bin2hex / hex2bin (invalid input returns false; not exercised here
//     because stock PHP also emits a warning we don't reproduce) ---
var_dump(bin2hex("Hi!"));
var_dump(hex2bin("486921"));

// --- nl2br ---
var_dump(nl2br("a\nb"));
var_dump(nl2br("a\r\nb"));
var_dump(nl2br("a\nb", false));

// --- strtr (3-arg char form, 2-arg array form) ---
var_dump(strtr("Hello", "el", "ip"));
var_dump(strtr("abc", "abcd", "xy"));
var_dump(strtr("aaa", "aa", "xy"));
var_dump(strtr("Hello World", array("Hello" => "Hi", "World" => "Earth")));
var_dump(strtr("abcd", array("ab" => "X", "abc" => "Y")));

// --- substr_replace ---
var_dump(substr_replace("Hello", "XX", 1, 2));
var_dump(substr_replace("Hello", "XX", 1));
var_dump(substr_replace("Hello", "XX", -2, 1));
var_dump(substr_replace("Hello", "XX", 2, -1));
var_dump(substr_replace("abc", "X", 1, 0));

// --- quotemeta ---
var_dump(quotemeta("1+1=2 (yes)"));
var_dump(quotemeta("a.b*c"));

// --- addslashes / stripslashes ---
$slashed = addslashes("a'b\"c\\d");
var_dump($slashed);
var_dump(stripslashes($slashed));
var_dump(bin2hex(addslashes("a\0b")));

// --- number_format ---
var_dump(number_format(1234.5678));
var_dump(number_format(1234.5678, 2));
var_dump(number_format(-1234.5678, 2));
var_dump(number_format(1234.5678, 2, ".", " "));
var_dump(number_format(0.005, 2));
var_dump(number_format(1000000, 0));
var_dump(number_format(2.5, 0));
var_dump(number_format(-2.5, 0));
var_dump(number_format(99.995, 2));

// --- str_word_count (mode 0, 1, 2) ---
var_dump(str_word_count("Hello world foo"));
var_dump(str_word_count("It's a test-case", 1));
var_dump(str_word_count("hi there world", 2));

// --- sprintf ---
var_dump(sprintf("%d", 42));
var_dump(sprintf("%5d", 42));
var_dump(sprintf("%-5d|", 42));
var_dump(sprintf("%05d", 42));
var_dump(sprintf("%+d", 42));
var_dump(sprintf("%+d", -42));
var_dump(sprintf("%s", "hi"));
var_dump(sprintf("%10s", "hi"));
var_dump(sprintf("%-10s|", "hi"));
var_dump(sprintf("%'*10s", "hi"));
var_dump(sprintf("%.3s", "hello"));
var_dump(sprintf("%f", 3.14159));
var_dump(sprintf("%.2f", 3.14159));
var_dump(sprintf("%8.2f", 3.14159));
var_dump(sprintf("%08.2f", -3.14));
var_dump(sprintf("%x", 255));
var_dump(sprintf("%X", 255));
var_dump(sprintf("%o", 8));
var_dump(sprintf("%b", 5));
var_dump(sprintf("%c", 65));
var_dump(sprintf("%u", -1));
var_dump(sprintf("%e", 12345.678));
var_dump(sprintf("%E", 12345.678));
var_dump(sprintf("%.2e", 12345.678));
var_dump(sprintf("%g", 0.00001234));
var_dump(sprintf("%g", 100000.0));
var_dump(sprintf("%g", 1000000.0));
var_dump(sprintf("%G", 123456789.0));
var_dump(sprintf("%%"));
var_dump(sprintf("%2\$s %1\$s", "a", "b"));
var_dump(sprintf("%5.2f%%", 12.3));

// --- vsprintf ---
var_dump(vsprintf("%s-%d", array("a", 5)));
var_dump(vsprintf("%2\$s-%1\$s", array("x", "y")));

// --- printf (writes output, returns byte length) ---
$written = printf("n=%d\n", 42);
echo "printf returned ", $written, "\n";
