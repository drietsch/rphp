<?php

// ip2long() is inet_pton(AF_INET) and just as strict: four decimal parts,
// each below 256, and nothing else.
foreach (['192.168.1.1', '0.0.0.0', '255.255.255.255', '256.1.1.1', '1.2.3',
          '1.2.3.4.5', '1.2.3.04', '01.2.3.4', ' 1.2.3.4', '1.2.3.4 ',
          '::1', '', 'localhost'] as $ip) {
    printf("%-18s %s\n", json_encode($ip), var_export(ip2long($ip), true));
}

// long2ip() takes the number modulo 2^32.
foreach ([0, 1, 3232235777, -1, 4294967295, 4294967296, -3232235777] as $n) {
    printf("%-12d %s\n", $n, long2ip($n));
}

// The packed forms, both families.
// (A scoped address, `fe80::1%lo0`, is left out: php hands it to the
// platform's inet_pton, and this box's answer is its own.)
foreach (['192.168.1.1', '::1', '2001:db8::1', '::ffff:192.168.1.1',
          'bogus'] as $ip) {
    $packed = @inet_pton($ip);
    printf("%-20s %s %s\n", json_encode($ip),
        $packed === false ? 'false' : bin2hex($packed),
        $packed === false ? '-' : inet_ntop($packed));
}
var_dump(@inet_ntop('xx'), @inet_ntop(''), inet_ntop(str_repeat("\0", 16)));

// fileinode() and stream_is_local(), the two filesystem answers that come
// from the wrapper rather than from the file.
$tmp = tempnam(sys_get_temp_dir(), 'rphp-net');
var_dump(fileinode($tmp) > 0, fileinode($tmp) === fileinode($tmp));
var_dump(@fileinode($tmp . '-nope'));
unlink($tmp);

foreach (['/tmp/x', 'file:///tmp/x', 'http://example.com/x', 'php://memory',
          'data:text/plain,x', 'relative/path', '', 'compress.zlib:///tmp/x',
          'ftp://x/y', 'FILE:///tmp/x', 'phar:///tmp/x.phar',
          'https://example.com'] as $path) {
    printf("%-26s %s\n", json_encode($path),
        var_export(stream_is_local($path), true));
}
$h = fopen('php://memory', 'r+');
var_dump(stream_is_local($h), stream_is_local(STDOUT));
fclose($h);

// The scandir sort orders, which are constants php resolves before the call.
var_dump(SCANDIR_SORT_ASCENDING, SCANDIR_SORT_DESCENDING, SCANDIR_SORT_NONE);
$dir = sys_get_temp_dir() . '/rphp-net-dir';
@mkdir($dir);
foreach (['b', 'a', 'c'] as $n) {
    file_put_contents("$dir/$n", '');
}
var_dump(scandir($dir), scandir($dir, SCANDIR_SORT_DESCENDING),
    count(scandir($dir, SCANDIR_SORT_NONE)));
foreach (['a', 'b', 'c'] as $n) {
    unlink("$dir/$n");
}
rmdir($dir);
