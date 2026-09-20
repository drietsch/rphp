<?php
// `stream_set_chunk_size` reports the previous size and refuses 0 or an
// int beyond 32 bits; `stream_set_write_buffer` is -1 unless the stream
// sits on a `FILE*` (only the standard handles); `stream_set_read_buffer`
// is the stream layer's own and always 0.
$f = fopen("php://memory","w+"); var_dump(stream_set_chunk_size($f, 100), stream_set_chunk_size($f, 200));
try { stream_set_chunk_size($f, 0);} catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { stream_set_chunk_size($f, PHP_INT_MAX);} catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(stream_set_write_buffer($f, 0), stream_set_read_buffer($f, 0), stream_set_write_buffer(STDOUT, 8192));
$s=fopen("/etc/hosts", "r"); var_dump(stream_set_write_buffer($s, 0), stream_set_read_buffer($s, 100), stream_set_write_buffer($s, 100));
var_dump(stream_set_read_buffer(STDIN, 0), stream_set_write_buffer(STDOUT, 0), stream_set_write_buffer(STDERR, 0), stream_set_write_buffer($s, -1));
