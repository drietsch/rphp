<?php
// Tier-A differential: mail()'s header validation — every case here is
// refused before the delivery program would run, so nothing is sent.

foreach (["SMTP", "smtp_port", "sendmail_from", "sendmail_path", "mail.add_x_header",
          "mail.log", "mail.force_extra_parameters", "mail.mixed_lf_and_crlf"] as $k) {
    var_dump(ini_get($k));
}

$cases = [
    [0 => "x"],
    ["To" => "x"],
    ["subject" => "x"],
    ["From" => ["a", "b"]],
    ["X" => 1],
    ["X" => null],
    ["X" => new stdClass],
    ["X" => ["k" => "v"]],
    ["X" => [1]],
    ["X Y" => "v"],
    ["X:" => "v"],
    ["X" => "v\r\nY: z"],
    ["X" => "v\r\n"],
    ["X" => "v\r"],
    ["X" => "v\n"],
    ["X" => "v\0w"],
    ["X" => ["ok", "bad\n"]],
    ["X-Fine" => "a", "Bcc" => ["x"]],
];
foreach (["orig-date", "sender", "reply-to", "cc", "bcc", "message-id", "in-reply-to", "references"] as $h) {
    $cases[] = [$h => ["a", "b"]];
    $cases[] = [strtoupper($h) => 5];
}
foreach ($cases as $i => $headers) {
    try {
        var_dump(mail("a@example.invalid", "S", "B", $headers));
    } catch (\Throwable $e) {
        echo "$i ", get_class($e), ": ", $e->getMessage(), "\n";
    }
}

// Malformed string headers: a leading newline or an empty line.
foreach (["\r\nX: y", "X: y\r\n\r\nZ: w", "X: y\n\nZ", " X: y", ":X", "X: y\r\rZ"] as $h) {
    var_dump(mail("a@example.invalid", "S", "B", $h));
}
// An array header whose name is empty builds ": v", which is malformed too.
var_dump(mail("a@example.invalid", "S", "B", ["" => "v"]));

try {
    mail("a", "s", "b", 1.5);
} catch (\Throwable $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
try {
    mail("a", "s");
} catch (\Throwable $e) {
    echo get_class($e), ": ", $e->getMessage(), "\n";
}
