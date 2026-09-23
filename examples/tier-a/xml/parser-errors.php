<?php
// ext/xml: libxml2's error numbers, php's compat-layer error strings, the
// position a failed parse is left at, and the API's error paths.
function xpos($p) {
    return "[" . xml_get_current_line_number($p) . ":" . xml_get_current_column_number($p)
        . "@" . xml_get_current_byte_index($p) . "]";
}
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
$cases = [
    "<a></b>", "<a>", "<a/><b/>", "", "   ", "abcd", "<a>&</a>", "<a x='1' x='2'/>", "<a>&foo;</a>",
    "<a x=1/>", "<a", "<a/>\n<?xml version='1.0'?>", "<1a/>", "<a><!-- x</a>", "<a><![CDATA[x</a>",
    "<a>&#0;</a>", "<a>&#x;</a>", "<a>&#q;</a>", "<a x='<'/>", "<a>\n\n  </b>", "<a>x]]>y</a>", "<a>\x01</a>",
    "<a>\xff</a>", "<a b='\xe9'/>", "<?xml version='1.0' encoding='bogus'?><a/>", "<a b='x' c/>", "</a>",
    "<a><b></a>", "<a>text", "<a>x", "<a>&amp</a>", "<a/>j", "<a/>jk", "<?xml version='2.0'?><a/>",
    "<?xml version='1.1'?><a/>", "<?xml version='1.0' standalone='maybe'?><a/>", "<a><!-- a--b --></a>",
    "<!DOCTYPE a SYSTEM 'x.dtd'><a>&foo;</a>",
];
foreach ($cases as $x) {
    foreach ([false, true] as $ns) {
        $p = $ns ? xml_parser_create_ns() : xml_parser_create();
        $r = xml_parse($p, $x, true);
        $c = xml_get_error_code($p);
        echo $ns ? "ns " : "", json_encode($x), " => $r code=$c ", json_encode(xml_error_string($c)), " ", xpos($p), "\n";
    }
}
foreach (["<p:a/>", "<a xmlns:p=''/>", "<r xmlns:p='urn:p' xmlns:q='urn:p' p:a='1' q:a='2'/>"] as $x) {
    $p = xml_parser_create_ns();
    echo json_encode($x), " => ", xml_parse($p, $x, true), " ", xml_get_error_code($p), "\n";
}
for ($i = 0; $i <= 102; $i++) {
    echo $i, ": ", xml_error_string($i), "\n";
}
var_dump(xml_error_string(-1), xml_error_string(201), XML_ERROR_TAG_MISMATCH, XML_SAX_IMPL);

echo "--- after an error\n";
$p = xml_parser_create();
var_dump(xml_parse($p, "<a></b>", false), xml_parse($p, "<c/>", true), xml_get_error_code($p));
$p = xml_parser_create();
var_dump(xml_parse($p, "<a/>", true), xml_parse($p, "<b/>", true), xml_get_error_code($p));

echo "--- the API\n";
$p = xml_parser_create();
t(fn() => xml_parser_set_option($p, XML_OPTION_TARGET_ENCODING, "bogus"));
t(fn() => xml_parser_set_option($p, 99, 1));
t(fn() => xml_parser_get_option($p, 99));
t(fn() => xml_parser_set_option($p, XML_OPTION_SKIP_TAGSTART, -1));
t(fn() => xml_parser_set_option($p, XML_OPTION_CASE_FOLDING, []));
t(fn() => xml_parser_get_option($p, XML_OPTION_CASE_FOLDING));
t(fn() => xml_parser_set_option($p, XML_OPTION_PARSE_HUGE, true));
t(fn() => xml_parser_get_option($p, XML_OPTION_PARSE_HUGE));
t(fn() => xml_parser_create("bogus"));
t(fn() => get_class(xml_parser_create("utf-8")));
t(fn() => xml_parse("x", "<a/>"));
t(fn() => xml_get_error_code(null));
t(fn() => xml_parser_free($p));
t(fn() => xml_parse($p, "<a/>"));
t(fn() => new XMLParser());
t(fn() => clone $p);
t(fn() => xml_set_element_handler($p, "nope", null));
t(fn() => xml_set_character_data_handler($p, 5));
t(fn() => xml_set_character_data_handler($p, [1, 2]));
t(fn() => xml_set_character_data_handler($p, "strlen"));
t(fn() => xml_set_default_handler($p, null));

echo "--- handlers as methods\n";
class H {
    function s($p, $n, $a) { echo "H::s $n\n"; }
    function e($p, $n) { echo "H::e $n\n"; }
    function c($p, $d) { echo "H::c $d\n"; }
}
$q = xml_parser_create();
t(fn() => xml_set_object($q, new H));
t(fn() => xml_set_element_handler($q, "s", "e"));
t(fn() => xml_set_character_data_handler($q, "c"));
t(fn() => xml_set_character_data_handler($q, "zz"));
t(fn() => xml_parse($q, "<a>t</a>", true));
$q = xml_parser_create();
t(fn() => xml_set_element_handler($q, [new H, 's'], [new H, 'e']));
t(fn() => xml_parse($q, "<a>t</a>", true));

echo "--- a throwing handler, a recursive parse\n";
$q = xml_parser_create();
xml_set_element_handler($q, function ($p, $n) { throw new Exception("boom $n"); }, null);
t(fn() => xml_parse($q, "<a><b/></a>", true));
$q = xml_parser_create();
xml_set_element_handler($q, function ($p, $n) {
    echo "in $n\n";
    xml_parse($p, "<z/>");
}, null);
t(fn() => xml_parse($q, "<a><b/></a>", true));
