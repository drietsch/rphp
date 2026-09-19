<?php
// `stat()`, `lstat()` and `fstat()`: php's 26-entry buffer — thirteen values
// under the numeric keys and the same thirteen again under php's names — plus
// the two spellings of the failure warning. Nothing machine-specific is
// printed: the shape and the facts derived from it are, the inode and the
// times are not.
$dir = sys_get_temp_dir() . '/rphp-fs-stat';
@mkdir($dir);
$p = $dir . '/f.txt';
file_put_contents($p, '0123456789');

$s = stat($p);
var_dump(count($s));
var_dump(array_keys($s));
var_dump(array_unique(array_map('gettype', $s)));

// The named half repeats the numeric half, in the same order.
var_dump(array_values(array_slice($s, 13, null, true)) === array_slice(array_values($s), 0, 13));
var_dump($s['blksize'] === $s[11], $s['blocks'] === $s[12]);

// The values agree with the single-field accessors.
var_dump($s['size'] === filesize($p));
var_dump($s['mtime'] === filemtime($p));
var_dump($s['mode'] === fileperms($p));
var_dump($s['uid'] === fileowner($p), $s['gid'] === filegroup($p));
var_dump($s['nlink'], $s['rdev'], decoct($s['mode'] & 0170000));

// `lstat()` does not follow the link, so its size is the length of the target
// string and its type nibble is `120000`.
@symlink($p, $dir . '/l');
$l = lstat($dir . '/l');
var_dump(count($l), $l['size'] === strlen($p), decoct($l['mode'] & 0170000));
var_dump(stat($dir . '/l')['size'] === filesize($p));
var_dump(decoct(stat($dir)['mode'] & 0170000));

// `fstat()` on a real handle is the stat of its file.
$h = fopen($p, 'rb');
$f = fstat($h);
var_dump(array_keys($f) === array_keys($s), count($f));
var_dump($f['size'] === filesize($p), $f['ino'] === $s['ino'], $f['mode'] === $s['mode']);
fclose($h);

// On a `php://` handle php answers a synthetic buffer: device 12
// (`/dev/null`), no inode, mode `0100666`, and -1 where the number makes no
// sense. It leaves the cursor and the end-of-file flag alone.
$m = fopen('php://memory', 'w+b');
fwrite($m, 'abcd');
$fm = fstat($m);
var_dump($fm['dev'], $fm['ino'], decoct($fm['mode']), $fm['nlink']);
var_dump($fm['uid'], $fm['gid'], $fm['rdev'], $fm['size']);
var_dump($fm['atime'], $fm['mtime'], $fm['ctime'], $fm['blksize'], $fm['blocks']);
var_dump(ftell($m), feof($m));
fclose($m);
// A handle that cannot be written reports `0100444` instead.
$r = fopen('php://memory', 'rb');
var_dump(decoct(fstat($r)['mode']));
fclose($r);

// The failure warnings. php capitalises `Lstat` and no other.
var_dump(@stat($dir . '/nope'), error_get_last()['message']);
var_dump(@lstat($dir . '/nope'), error_get_last()['message']);
var_dump(@fileowner($dir . '/nope'), error_get_last()['message']);
var_dump(@filegroup($dir . '/nope'), error_get_last()['message']);
var_dump(stat($dir . '/nope'));

var_dump(unlink($dir . '/l'), unlink($p), rmdir($dir));
