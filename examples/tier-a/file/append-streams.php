<?php
// fopen(…, "a") writes straight to the file (nothing read, nothing lost
// between writers); ftell counts the bytes written; "a+" reads from the
// start and appends.
$f = sys_get_temp_dir() . '/rphp-ap-' . getmypid() . '.txt';
file_put_contents($f, "hello\n");
$h = fopen($f, 'a'); var_dump(ftell($h)); var_dump(fwrite($h, "x\n")); var_dump(ftell($h));
var_dump(@fread($h, 10)); fseek($h, 0); var_dump(ftell($h)); fwrite($h, "y\n"); var_dump(ftell($h));
var_dump(fflush($h)); var_dump(file_get_contents($f)); fclose($h);
var_dump(file_get_contents($f));
$h = fopen($f, 'a+'); var_dump(ftell($h), fread($h, 3)); fwrite($h, "z\n"); fclose($h); var_dump(file_get_contents($f));
$h = fopen($f . '.new', 'a'); var_dump(file_exists($f . '.new')); fclose($h); var_dump(filesize($f . '.new'));
var_dump(stream_get_meta_data(fopen($f, 'a'))['mode']);
unlink($f); unlink($f . '.new');
