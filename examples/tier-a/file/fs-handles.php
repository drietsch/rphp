<?php
// The handle operations: `flock()`, `fsync()`/`fdatasync()`, `fpassthru()`
// and `fscanf()`. php reads `flock()`'s operation out of the low two bits, so
// `flock($f, 99)` is a perfectly good `LOCK_UN`, and it clears `$wouldblock`
// before every attempt.
$dir = sys_get_temp_dir() . '/rphp-fs-handles';
@mkdir($dir);
$p = $dir . '/f.txt';
file_put_contents($p, "12 abc\n34 def\n");

var_dump(LOCK_SH, LOCK_EX, LOCK_UN, LOCK_NB);

$a = fopen($p, 'rb+');
$b = fopen($p, 'rb+');
var_dump(flock($a, LOCK_EX));

// Two handles on one file conflict, and the non-blocking form says why.
$w = 'untouched';
var_dump(flock($b, LOCK_EX | LOCK_NB, $w), $w);
var_dump(flock($b, LOCK_SH | LOCK_NB, $w), $w);

// Downgrading to a shared lock lets the second handle in.
var_dump(flock($a, LOCK_SH));
var_dump(flock($b, LOCK_SH | LOCK_NB, $w), $w);
var_dump(flock($b, LOCK_UN), flock($a, LOCK_UN));

// The operation is `$operation & 3`.
var_dump(flock($a, 99));
try {
    flock($a, 0);
} catch (Throwable $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}

// A read-only handle locks just as well; a handle with no file does not.
$r = fopen($p, 'rb');
var_dump(flock($r, LOCK_EX), flock($r, LOCK_UN));
fclose($r);
$m = fopen('php://memory', 'w+b');
var_dump(flock($m, LOCK_EX));

// `fsync()` needs a file behind the handle.
var_dump(@fsync($m), error_get_last()['message']);
var_dump(@fdatasync($m), error_get_last()['message']);
fclose($m);
fwrite($a, 'X');
var_dump(fsync($a), fdatasync($a));
fclose($a);
fclose($b);
var_dump(file_get_contents($p));

// `fpassthru()` writes what is left of the handle and leaves it at the end.
$h = fopen($p, 'rb');
var_dump(fread($h, 3));
var_dump(fpassthru($h));
echo "\n";
var_dump(feof($h), ftell($h));
var_dump(fpassthru($h), feof($h));
fclose($h);

// Its failure answer is `-1`, php's C return, not `false`.
$wo = fopen($dir . '/w.txt', 'wb');
var_dump(@fpassthru($wo), error_get_last()['message']);
fclose($wo);

// `fscanf()` is one line plus `sscanf()`: the cursor ends past the newline.
file_put_contents($p, "12 abc\n34 def\n");
$h = fopen($p, 'rb');
var_dump(fscanf($h, '%d %s'));
var_dump(ftell($h), feof($h));
var_dump(fscanf($h, '%d %s'));
var_dump(fscanf($h, '%d %s'));
fclose($h);

// With variables it answers how many it filled.
$h = fopen($p, 'rb');
var_dump(fscanf($h, '%d %s', $n, $s), $n, $s);
var_dump(fscanf($h, '%s %s %s'));
fclose($h);

$wo = fopen($dir . '/w.txt', 'wb');
var_dump(@fscanf($wo, '%d'), error_get_last()['message']);
fclose($wo);

var_dump(unlink($p), unlink($dir . '/w.txt'), rmdir($dir));
