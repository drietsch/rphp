<?php
// The transports beside tcp: udp datagrams, a unix-domain stream, and the
// anonymous pair. Again nothing prints a port or a temp path.

echo "--- udp ---\n";
$s = stream_socket_server('udp://127.0.0.1:0', $e, $es, STREAM_SERVER_BIND);
var_dump($s !== false, $e, $es);
$port = explode(':', stream_socket_get_name($s, false))[1];
$c = stream_socket_client("udp://127.0.0.1:$port", $e2, $es2);
var_dump(stream_socket_sendto($c, 'hello'));
var_dump(stream_socket_recvfrom($s, 16, 0, $from));
var_dump(explode(':', $from)[0]);
var_dump(stream_get_meta_data($s)['stream_type']);
fclose($c);
fclose($s);

echo "--- unix ---\n";
$path = sys_get_temp_dir() . '/rphp-u' . getmypid() . '.sock';
@unlink($path);
$u = stream_socket_server("unix://$path", $e3, $es3);
var_dump($u !== false, $e3, $es3);
var_dump(stream_socket_get_name($u, false) === $path);
$uc = stream_socket_client("unix://$path", $e4, $es4);
var_dump($uc !== false);
$ua = stream_socket_accept($u, 2);
var_dump($ua !== false);
$m = stream_get_meta_data($uc);
var_dump($m['stream_type'], $m['uri'] === "unix://$path");
fwrite($uc, "over the socket\n");
var_dump(fgets($ua));
fclose($ua);
fclose($uc);
fclose($u);
@unlink($path);

echo "--- pair ---\n";
$p = stream_socket_pair(STREAM_PF_UNIX, STREAM_SOCK_STREAM, 0);
var_dump(is_array($p), count($p), get_resource_type($p[0]));
fwrite($p[0], 'ab');
var_dump(fread($p[1], 2));
$m = stream_get_meta_data($p[0]);
// A pair has no address at all, so php's metadata carries no `uri` key.
var_dump($m['stream_type'], array_key_exists('uri', $m));

// A delimiter of any length, which stream_get_line() strips.
fwrite($p[0], 'aa--bb--cc');
fclose($p[0]);
var_dump(stream_get_line($p[1], 100, '--'));
var_dump(stream_get_line($p[1], 100, '--'));
var_dump(stream_get_line($p[1], 100, '--'));
var_dump(stream_get_line($p[1], 100, '--'));
fclose($p[1]);
