<?php
// A read timeout is not end-of-stream: php reports `timed_out` with `eof`
// still false, and the next read that gets somewhere clears the flag.

$p = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);

var_dump(stream_set_timeout($p[1], 0, 150000));
$started = microtime(true);
var_dump(fgets($p[1]));
$m = stream_get_meta_data($p[1]);
var_dump($m['timed_out'], $m['eof']);
// It waited, but it did not hang.
var_dump(microtime(true) - $started < 3);

fwrite($p[0], "now\n");
var_dump(fgets($p[1]));
var_dump(stream_get_meta_data($p[1])['timed_out']);

echo "--- select ---\n";
// Nothing pending: select comes back with an empty set, not a block.
$r = [$p[1]];
$w = null;
$x = null;
var_dump(stream_select($r, $w, $x, 0, 0), count($r));

fwrite($p[0], "data\n");
$r = [$p[1]];
$w = null;
$x = null;
var_dump(stream_select($r, $w, $x, 1, 0), count($r));
var_dump(fgets($p[1]));

echo "--- eof ---\n";
// The peer going away is end-of-stream, and feof() says so before a read.
fclose($p[0]);
var_dump(feof($p[1]));
var_dump(fread($p[1], 8));
var_dump(feof($p[1]));
fclose($p[1]);

echo "--- non-blocking ---\n";
$q = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
var_dump(stream_set_blocking($q[1], false));
var_dump(stream_get_meta_data($q[1])['blocked']);
// Nothing there yet is the empty string, and *not* a timeout.
var_dump(fread($q[1], 4));
var_dump(stream_get_meta_data($q[1])['timed_out']);
fclose($q[0]);
fclose($q[1]);
