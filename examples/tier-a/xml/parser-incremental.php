<?php
// ext/xml: feeding a document in pieces — what each xml_parse() call may
// consume (a start tag waits for its '>', text for the next '<').
function xpos($p) {
    return "[" . xml_get_current_line_number($p) . ":" . xml_get_current_column_number($p)
        . "@" . xml_get_current_byte_index($p) . "]";
}
function parser() {
    $p = xml_parser_create();
    xml_set_element_handler(
        $p,
        function ($p, $n, $a) { echo xpos($p), " start $n\n"; },
        function ($p, $n) { echo xpos($p), " end $n\n"; }
    );
    xml_set_character_data_handler($p, function ($p, $d) { echo xpos($p), " cdata ", json_encode($d), "\n"; });
    xml_set_default_handler($p, function ($p, $d) { echo xpos($p), " default ", json_encode($d), "\n"; });
    return $p;
}

$p = parser();
foreach (["<ro", "ot>he", "llo <b>x", "</b>", "tail", "</root>"] as $chunk) {
    echo "chunk ", json_encode($chunk), " => ", xml_parse($p, $chunk, false), " ", xpos($p), "\n";
}
echo "final => ", xml_parse($p, "", true), " ", xml_get_error_code($p), " ", xpos($p), "\n";

echo "--- byte by byte\n";
$p = parser();
$doc = "<?xml version='1.0'?><root a='1'>te&amp;xt<!-- c --><b>x</b>\n<![CDATA[cd]]></root>";
for ($i = 0; $i < strlen($doc); $i++) {
    if (!xml_parse($p, $doc[$i], false)) {
        echo "failed at $i\n";
        break;
    }
}
echo "final => ", xml_parse($p, "", true), " ", xml_get_error_code($p), " ", xpos($p), "\n";

echo "--- an error in the middle\n";
$p = parser();
var_dump(xml_parse($p, "<a><b>", false));
var_dump(xml_parse($p, "</c>", false));
var_dump(xml_parse($p, "</a>", true), xml_get_error_code($p), xpos($p));

echo "--- a long text\n";
$p = xml_parser_create();
xml_set_character_data_handler($p, function ($p, $d) { echo xpos($p), " ", strlen($d), "\n"; });
xml_parse($p, "<a>" . str_repeat("abcdefghij", 50), false);
xml_parse($p, str_repeat("klmnopqrst", 50) . "</a>", true);
