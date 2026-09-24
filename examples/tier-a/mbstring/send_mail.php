<?php
// mb_send_mail's argument checks, all raised before anything is sent
// (the oracle cannot redirect sendmail_path, so no mail leaves here).
$cases = [
    fn() => mb_send_mail("foo\0bar", "x", "y"),
    fn() => mb_send_mail("x", "foo\0bar", "y"),
    fn() => mb_send_mail("x", "y", "foo\0bar"),
    fn() => mb_send_mail("x", "y", "z", "foo\0bar"),
    fn() => mb_send_mail("x", "y", "z", "q", "foo\0bar"),
    fn() => mb_send_mail("a", "b", "c", ["Cc" => ["a", "b"]]),
    fn() => mb_send_mail("a", "b", "c", ["X" => 1]),
    fn() => mb_send_mail("a", "b", "c", ["X" => null]),
    fn() => mb_send_mail("a", "b", "c", ["X" => "a\nb"]),
    fn() => mb_send_mail("a", "b", "c", ["X" => "a\rb"]),
    fn() => mb_send_mail("a", "b", "c", ["X" => ["a", 1]]),
    fn() => mb_send_mail("a", "b", "c", ["X Y" => "a"]),
    fn() => mb_send_mail("a", "b", "c", ["X:" => "a"]),
    fn() => mb_send_mail("a", "b", "c", ["From" => "a\r\nb"]),
    fn() => mb_send_mail("a", "b", "c", ["To" => "x"]),
    fn() => mb_send_mail("a", "b", "c", ["subject" => "x"]),
    fn() => mb_send_mail("a", "b", "c", [1 => "x"]),
];
foreach ($cases as $f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
