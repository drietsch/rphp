<?php
// proc_open with the three pipes, the way Symfony's Process drives it:
// non-blocking reads under stream_select, proc_get_status, proc_close.
$spec = [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => ['pipe', 'w']];
$p = proc_open(['/bin/sh', '-c', 'read line; echo "got: $line"; echo "oops" >&2; exit 3'], $spec, $pipes, '/tmp', ['HOME' => '/nowhere', 'PATH=/usr/bin:/bin']);
var_dump(is_resource($p), get_resource_type($p));
var_dump(count($pipes), get_resource_type($pipes[1]));
$status = proc_get_status($p);
var_dump($status['running'], is_int($status['pid']), $status['command']);
fwrite($pipes[0], "hello\n");
fclose($pipes[0]);
stream_set_blocking($pipes[1], false);
stream_set_blocking($pipes[2], false);
var_dump(stream_get_meta_data($pipes[1])['blocked']);
$out = $err = '';
$open = [1 => $pipes[1], 2 => $pipes[2]];
$deadline = microtime(true) + 5;
while ($open && microtime(true) < $deadline) {
    $r = $open;
    $w = $e = null;
    $n = stream_select($r, $w, $e, 0, 200000);
    if ($n === false) {
        break;
    }
    foreach ($r as $k => $s) {
        $chunk = fread($s, 8192);
        if ($k === 1) {
            $out .= $chunk;
        } else {
            $err .= $chunk;
        }
        if (feof($s)) {
            fclose($s);
            unset($open[$k]);
        }
    }
}
var_dump($out, $err);
$code = proc_close($p);
var_dump($code);

// A string command runs through the shell; the exit code comes back twice
// (php 8.3+ keeps reporting it after the first proc_get_status).
$p = proc_open('exit 7', [], $pipes);
usleep(50000);
$s = proc_get_status($p);
while ($s['running']) {
    usleep(10000);
    $s = proc_get_status($p);
}
var_dump($s['exitcode'], proc_get_status($p)['exitcode'], proc_close($p));

// A missing program: a warning and false.
var_dump(@proc_open(['/no/such/binary'], $spec, $pipes));

// A descriptor to a file, and the child's cwd.
$tmp = tempnam(sys_get_temp_dir(), 'po');
$p = proc_open(['/bin/pwd'], [1 => ['file', $tmp, 'w']], $pipes, '/');
proc_close($p);
var_dump(trim(file_get_contents($tmp)));
unlink($tmp);

// proc_terminate on a sleeping child.
$p = proc_open(['/bin/sleep', '30'], [1 => ['pipe', 'w']], $pipes);
var_dump(proc_terminate($p));
usleep(100000);
$s = proc_get_status($p);
var_dump($s['running'], $s['signaled'], $s['termsig'], $s['exitcode']);
fclose($pipes[1]);
var_dump(proc_close($p));
