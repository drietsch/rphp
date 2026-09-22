<?php
// php's stat cache is one slot, and *any* call that changes a path empties
// it — not just one that names the cached path. So a directory moved out
// from under a path already stat'ed is seen to be gone on the next look,
// which is what Symfony's cache-directory dance depends on.
$base = sys_get_temp_dir() . '/sc-' . getmypid();
@mkdir($base);
mkdir($base . '/a/b', 0777, true);
var_dump(is_dir($base . '/a/b'));      // the slot holds a/b
mkdir($base . '/unrelated');            // a change to another path…
rename($base . '/a', $base . '/a2');    // …and one whose arguments are not a/b
var_dump(is_dir($base . '/a/b'));       // the parent moved: gone
var_dump(is_dir($base . '/a2/b'));      // and it is over here
rmdir($base . '/unrelated');
// the same for a file the script writes and then asks about
$f = $base . '/f.txt';
var_dump(file_exists($f));
file_put_contents($f, "x");
var_dump(file_exists($f), filesize($f));
unlink($f);
var_dump(file_exists($f));
rmdir($base . '/a2/b'); rmdir($base . '/a2'); rmdir($base);
