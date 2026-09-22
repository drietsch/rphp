<?php
// php's stat cache holds exactly one path — the last one looked at — so a
// second stat drops the first, and an entry can never survive its own
// directory being moved away. Symfony's cache-directory dance depends on it:
// the kernel stats var/cache/dev, the installer renames var/cache aside, and
// the next is_dir() must say the directory is gone.
$base = sys_get_temp_dir() . '/pm-' . getmypid();
@mkdir($base);
mkdir($base . '/cache/dev', 0777, true);
$dev = $base . '/cache/dev';
var_dump(is_dir($dev));                 // rphp caches "dev is a dir"
var_dump(is_dir($base));                // php's one-entry cache now holds $base
rename($base . '/cache', $base . '/cach~');
mkdir($base . '/cache');
var_dump(is_dir($dev));                 // the parent moved: dev is gone
@rmdir($base . '/cache'); @rmdir($base . '/cach~/dev'); @rmdir($base . '/cach~'); @rmdir($base);
