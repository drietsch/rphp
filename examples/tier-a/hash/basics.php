<?php
// Differential snippet for the `hash` extension. Every function is exercised and
// its result printed so rphp's output can be diffed byte-for-byte against stock
// PHP 8.5. hash_algos() is checked via membership (not printed whole) because
// stock PHP advertises far more algorithms than rphp implements.

// md5: hex + raw-binary (rendered through md5 again so the bytes stay printable).
echo md5(""), "\n";
echo md5("abc"), "\n";
echo md5("The quick brown fox jumped over the lazy dog."), "\n";
echo strlen(md5("abc", true)), "\n";
echo md5(md5("abc", true)), "\n";

// sha1: hex + raw-binary length and re-digest.
echo sha1(""), "\n";
echo sha1("abc"), "\n";
echo strlen(sha1("abc", true)), "\n";
echo sha1(sha1("abc", true)), "\n";

// crc32: plain integer (unsigned 32-bit value).
echo crc32(""), "\n";
echo crc32("abc"), "\n";
echo crc32("The quick brown fox jumped over the lazy dog."), "\n";

// hash(): every supported algorithm, hex form.
echo hash("md5", "abc"), "\n";
echo hash("sha1", "abc"), "\n";
echo hash("sha256", "abc"), "\n";
echo hash("sha384", "abc"), "\n";
echo hash("sha512", "abc"), "\n";
echo hash("crc32b", "abc"), "\n";
echo hash("crc32b", ""), "\n";

// hash() is case-insensitive in the algo name and supports binary output.
echo hash("SHA256", "abc"), "\n";
echo md5(hash("sha256", "abc", true)), "\n";

// hash_algos(): membership of each algorithm rphp supports (true in both engines).
$algos = hash_algos();
var_dump(in_array("md5", $algos));
var_dump(in_array("sha1", $algos));
var_dump(in_array("sha256", $algos));
var_dump(in_array("sha384", $algos));
var_dump(in_array("sha512", $algos));
var_dump(in_array("crc32b", $algos));
