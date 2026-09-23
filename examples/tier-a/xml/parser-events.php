<?php
// ext/xml: the handler events libxml2's push parser delivers, with the
// line / column / byte index each handler sees and how text is split.
function xpos($p) {
    return "[" . xml_get_current_line_number($p) . ":" . xml_get_current_column_number($p)
        . "@" . xml_get_current_byte_index($p) . "]";
}
function parser($ns = false) {
    $p = $ns ? xml_parser_create_ns() : xml_parser_create();
    xml_set_element_handler(
        $p,
        function ($p, $name, $attrs) { echo xpos($p), " start $name ", json_encode($attrs), "\n"; },
        function ($p, $name) { echo xpos($p), " end $name\n"; }
    );
    xml_set_character_data_handler($p, function ($p, $data) { echo xpos($p), " cdata ", json_encode($data), "\n"; });
    xml_set_processing_instruction_handler($p, function ($p, $target, $data) {
        echo xpos($p), " pi $target ", var_export($data, true), "\n";
    });
    xml_set_default_handler($p, function ($p, $data) { echo xpos($p), " default ", json_encode($data), "\n"; });
    return $p;
}

$doc = "<?xml version='1.0'?>\n<root a='1' B=\"x&amp;y\">\n  <item id='1'>Hello &amp; world &#65; &lt;&gt;</item>\n"
    . "  <!-- a comment -->\n  <?target some data?>\n  <![CDATA[ raw <x> ]]>\n  <empty/>\n"
    . " <ns:x xmlns:ns='urn:a'>t\r\nu</ns:x>\tz <?bare?></root>\n";
$p = parser();
var_dump(xml_parse($p, $doc, true));
echo xpos($p), " code=", xml_get_error_code($p), "\n";

echo "--- text pieces\n";
foreach ([
    "<a>h\xc3\xa9llo w\xc3\xb6rld</a>",
    "<a>x\ry\r\nz</a>",
    "<a>]x a]]b</a>",
    "<a>tab\there\n\n</a>",
    "<a><![CDATA[]]></a>",
    "<a b='1&#10;2\n3\t4'/>",
    "<a>" . str_repeat("\xc3\xa9", 200) . "</a>",
] as $x) {
    $p = xml_parser_create();
    xml_set_character_data_handler($p, function ($p, $d) { echo xpos($p), " ", strlen($d), " ", json_encode(substr($d, 0, 12)), "\n"; });
    xml_set_element_handler($p, function ($p, $n, $a) { echo xpos($p), " <$n> ", json_encode($a), "\n"; }, null);
    xml_parse($p, $x, true);
}

echo "--- case folding off\n";
$p = parser();
xml_parser_set_option($p, XML_OPTION_CASE_FOLDING, false);
xml_parse($p, "<Root Attr='v'><Child/></Root>", true);

echo "--- namespaces\n";
$p = parser(true);
xml_set_start_namespace_decl_handler($p, function ($p, $prefix, $uri) { echo xpos($p), " ns ", var_export($prefix, true), " $uri\n"; });
xml_set_end_namespace_decl_handler($p, function ($p, $prefix) { echo "never called\n"; });
var_dump(xml_parse($p, "<r xmlns='urn:d' xmlns:x='urn:x' a='1' x:b='2'><x:c/><d xmlns=''/></r>", true));
foreach (['#', '', '::'] as $sep) {
    $p = xml_parser_create_ns('UTF-8', $sep);
    xml_set_element_handler($p, function ($p, $n, $a) { echo "start $n ", json_encode($a), "\n"; }, function ($p, $n) { echo "end $n\n"; });
    xml_parse($p, "<p:r xmlns:p='urn:p' p:a='1' b='2'/>", true);
}
