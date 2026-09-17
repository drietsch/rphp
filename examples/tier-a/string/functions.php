<?php
// string.c, second half: C-slashes, chunking, paths, phonetics/distances,
// csv, rot13, spans, natural order, tokenizing, wordwrap, utf8_*, locale.

// --- addcslashes / stripcslashes ---
var_dump(addcslashes("foo[bar]\n\t\x00\x7f\xff\x1b\x07\x08\x0c\x0b\r", "A..Z\0..\37!@\177..\377"));
var_dump(addcslashes("a.b", "."), addcslashes("abc\\", "\\"), addcslashes("hello", ""));
var_dump(addcslashes("zoo[\"a\"]", "z..A"));      // warning: not incrementing
var_dump(addcslashes("abc", "a.."));               // warning: nothing to the right
var_dump(addcslashes("abc", "..c"));               // warning: nothing to the left
var_dump(stripcslashes("\\a\\b\\f\\n\\r\\t\\v\\\\\\x41\\x4\\101\\7\\z\\"));
var_dump(bin2hex(stripcslashes("\\x41\\x4g\\1010\\400\\xzz\\xFF")));

// --- chunk_split / count_chars ---
var_dump(chunk_split("abcdefg", 3, "|"), chunk_split("abcdef", 3, "|"), chunk_split("", 3, "|"), chunk_split("abc"), chunk_split("abc", 5), chunk_split("abc", 1, ""));
var_dump(count_chars("abca", 1), count_chars("abca", 3), strlen(count_chars("abca", 4)), count(count_chars("abca", 0)), count(count_chars("abca", 2)), count_chars("", 3));
$c = count_chars("ab", 0);
var_dump($c[97], $c[0]);

// --- dirname / basename / pathinfo ---
foreach (["/etc/passwd", "/etc/", "etc", ".", "..", "/", "//", "", "a/b/c/", "/a//b//", "///a///b///", "c:/x", "dir/.hidden"] as $p) {
    echo json_encode($p), " => ", json_encode(dirname($p)), " ", json_encode(dirname($p, 2)), " ", json_encode(basename($p)), " ", json_encode(basename($p, "c")), "\n";
}
var_dump(basename("/etc/sudoers.d", ".d"), basename("file.php", "file.php"), basename(".php", ".php"), basename("a.php.php", ".php"), basename("a/", "a"));
print_r(pathinfo("/www/htdocs/inc/lib.inc.php"));
print_r(pathinfo("/www/htdocs/inc/lib"));
print_r(pathinfo("lib.inc.php"));
print_r(pathinfo("/www/.htaccess"));
print_r(pathinfo("/www/dir/"));
print_r(pathinfo(""));
print_r(pathinfo("/a/b/c."));
var_dump(pathinfo("/a/b/c.txt", 4), pathinfo("/a/b/c", 4), pathinfo("/a/b/c.txt", 1), pathinfo("/a/b/c.txt", 2), pathinfo("/a/b/c.txt", 8), pathinfo("/a/b/c.txt", 3), pathinfo("/a/b/c.txt", 0));

// --- levenshtein / similar_text / soundex / metaphone ---
var_dump(levenshtein("kitten", "sitting"), levenshtein("", "abc"), levenshtein("abc", ""), levenshtein("a", "b", 2, 3, 4), levenshtein("abc", "abd", 1, 5, 1), levenshtein("abc", "ac", 1, 1, 7), levenshtein("ab", "abc", 3, 1, 1));
var_dump(similar_text("World", "Word"), similar_text("Hello", "World", $p), $p, similar_text("", "", $q), $q, similar_text("PHP IS GREAT", "WITH MYSQL", $s), $s, similar_text("abc", "", $r), $r);
foreach (["Robert", "Rupert", "Rubin", "Ashcraft", "Tymczak", "Pfister", "Honeyman", "", "123", "Lloyd", "Euler", "Knuth", "Lukasiewicz", "Wachs"] as $w) {
    echo soundex($w), " ";
}
echo "\n";
foreach (["Thompson", "knight", "Thumb", "Xavier", "Science", "school", "Wright", "Aebersold", "Gnagy", "Knuth", "Pfister", "Wachs", "ghost", "laugh", "Hugh", "dodge", "judge", "cia", "Caesar", "Chris", "Character", "Bach", "Michael", "Schmidt", "Thomas", "who", "Wynn", "Wylde", "Xerxes", "Zebra", "Yolanda", "Guillermo", "Guinness", "gnome", "Wrangler", "Aaron", "Isaac", "", "123abc", "comb", "Dumb", "Signed", "Tough", "Bough", "Hugh Grant", "nation", "this", "Sch", "cough", "Dayton", "BLB", "combo", "Schwartz", "match", "Ghislaine", "Whale", "Wy", "accent", "Xx", "Ax", "mbx", "HXGH", "GNEDx", "Tio", "Chy", "Knuth Knight", "thth", "cch"] as $w) {
    echo metaphone($w), " ";
}
echo "\n";
var_dump(metaphone("Philip", 3), metaphone("Hello world", 4));

// --- str_getcsv ---
var_dump(str_getcsv("a,\"b,c\",d", ",", "\"", "\\"));
var_dump(str_getcsv("\"a\"\"b\",\"c\\\"d\",e", ",", "\"", "\\"));
var_dump(str_getcsv("", ",", "\"", "\\"), str_getcsv("a,,c,", ",", "\"", "\\"));
var_dump(str_getcsv(" a , \"b\" , c ", ",", "\"", "\\"));
var_dump(str_getcsv("\"unterminated,x", ",", "\"", "\\"), str_getcsv("a;b", ";", "\"", ""));
var_dump(str_getcsv("\"a\\\"b\"", ",", "\"", ""), str_getcsv("\"a\\\"b\"", ",", "\"", "\\"));
var_dump(str_getcsv("\"a\" x,b", ",", "\"", "\\"), str_getcsv("\"a\"\"\",b", ",", "\"", "\\"));
var_dump(str_getcsv("\"a\nb\",c", ",", "\"", "\\"), str_getcsv("a,b\r\n", ",", "\"", "\\"));
var_dump(str_getcsv("a,b"));                        // deprecated: escape not given

// --- str_rot13 / strcoll / strspn / strcspn ---
var_dump(str_rot13("Hello, World!"), strcoll("a", "b"), strcoll("b", "a"), strcoll("a", "a"), strcoll("ab", "abc"));
var_dump(strspn("42 is the answer", "1234567890"), strspn("foo", "o", 1, 2), strspn("foo", "o", 1, -1), strspn("foo", "o", -2), strspn("abc", "abc", 5), strspn("abc", "abc", -5), strspn("abc", "", 0), strspn("abc", "abc", 0, -10));
var_dump(strcspn("abcd", "cd"), strcspn("abcd", "cd", -3), strcspn("abcd", "cd", 1, -2), strcspn("abcd", ""), strcspn("abcd", "x", 10), strcspn("abcd", "x", 2, 100), strcspn("hello", "l", -3, -1));

// --- strnatcmp / strnatcasecmp ---
$pairs = [["img12.png", "img10.png"], ["img2.png", "img10.png"], ["a01", "a1"], ["a1", "a01"], ["0.5", "0.10"], ["x 1", "x  1"], ["  a", "a"], ["", "a"], ["000", "00"], ["0", "00"], ["A", "a"], ["1", "1a"], ["10", "9"], ["a10b", "a9b"], ["0.001", "0.01"], ["x0y", "x00y"], ["07", "7"], ["1.001", "1.0001"], ["a  ", "a"]];
foreach ($pairs as $pr) {
    echo strnatcmp($pr[0], $pr[1]), " ", strnatcasecmp($pr[0], $pr[1]), " ";
}
echo strnatcasecmp("IMG2.png", "img10.PNG"), "\n";

// --- strtok ---
$tok = strtok("This is\tan example\nstring", " \n\t");
while ($tok !== false) {
    echo "[", $tok, "]";
    $tok = strtok(" \n\t");
}
echo "\n";
var_dump(strtok("", " "), strtok(" "), strtok("a,b", ","), strtok(","), strtok(","), strtok("x"));
var_dump(strtok(",,a,,b,,", ","), strtok(","), strtok(","), strtok("abc", ""), strtok(""));
var_dump(strtok("a b", " "), strtok("c d", " "), strtok(" "));

// --- substr_compare ---
echo substr_compare("abcde", "bc", 1, 2), " ", substr_compare("abcde", "de", -2), " ", substr_compare("abcde", "bcg", 1, 2), " ", substr_compare("abcde", "BC", 1, 2, true), " ", substr_compare("abcde", "bc", 1, 3), " ", substr_compare("abcde", "cd", 1, 2), " ", substr_compare("abcde", "abc", 0), " ", substr_compare("abcde", "", 0), "\n";
echo substr_compare("abcde", "x", 5), " ", substr_compare("abcde", "", 5), " ", substr_compare("abcde", "e", -10, 1), " ", substr_compare("abc", "abcd", 0), " ", substr_compare("ab", "abc", 0, 2), " ", substr_compare("Hello", "hello", 0, null, true), " ", substr_compare("abcde", "x", 0, 0), "\n";

// --- wordwrap ---
var_dump(wordwrap("The quick brown fox sat over the lazy dog", 15, "\n", true));
var_dump(wordwrap("A very long woooooooooooord.", 8, "\n", true));
var_dump(wordwrap("A very long woooooooooooooooooord. and something", 8, "\n", false));
var_dump(wordwrap("short", 10), wordwrap("", 5), wordwrap("a b c d", 1, "|", true), wordwrap("abc def", 0, "|"), wordwrap("abcdef", 2, "-", true), wordwrap("abc  def", 3, "|", true), wordwrap("abc def\nghi jkl", 5, "|"), wordwrap("12345", 3, "<br>", true), wordwrap("ab cd", 2, "XY", true), wordwrap(" ab", 2, "|", true), wordwrap("ab  cd", 2, "|", true), wordwrap("abcd efgh", 3, "|"), wordwrap("abcd efgh", 3, "|", true), wordwrap("x y", -1, "\n"));

// --- utf8_encode / utf8_decode (deprecated since 8.2) ---
var_dump(bin2hex(utf8_encode("\xe9\x80a")));
var_dump(bin2hex(utf8_decode("\xc3\xa9\xe2\x82\xac\xffa\xc3")));

// --- setlocale / localeconv (C locale only) ---
var_dump(setlocale(0, "0"), setlocale(0, "C"), setlocale(0, "POSIX"), setlocale(0, "xx_XX"), setlocale(0, ["xx_XX", "C"]), setlocale(0, "xx_XX", "C"), setlocale(2, "C"));
var_dump(localeconv());
