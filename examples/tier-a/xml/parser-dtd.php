<?php
// ext/xml: an internal subset's declarations and entity references —
// the notation / unparsed-entity / external-entity handlers, and where an
// internal entity's text goes with and without a default handler.
function xpos($p) {
    return "[" . xml_get_current_line_number($p) . ":" . xml_get_current_column_number($p)
        . "@" . xml_get_current_byte_index($p) . "]";
}
function parser($default) {
    $p = xml_parser_create();
    xml_set_element_handler($p, function ($p, $n, $a) { echo xpos($p), " start $n ", json_encode($a), "\n"; }, function ($p, $n) { echo xpos($p), " end $n\n"; });
    xml_set_character_data_handler($p, function ($p, $d) { echo xpos($p), " cdata ", json_encode($d), "\n"; });
    if ($default) {
        xml_set_default_handler($p, function ($p, $d) { echo xpos($p), " default ", json_encode($d), "\n"; });
    }
    xml_set_unparsed_entity_decl_handler($p, function ($p, ...$a) { echo "unparsed ", json_encode($a), "\n"; });
    xml_set_notation_decl_handler($p, function ($p, ...$a) { echo "notation ", json_encode($a), "\n"; });
    xml_set_external_entity_ref_handler($p, function ($p, ...$a) { echo "external ", json_encode($a), "\n"; return true; });
    xml_set_processing_instruction_handler($p, function ($p, $t, $d) { echo xpos($p), " pi $t ", json_encode($d), "\n"; });
    return $p;
}
$doc = "<!DOCTYPE r [\n<!ENTITY e 'ent'>\n<!ENTITY m '<b>x</b>'>\n<!ENTITY ext SYSTEM 'ext.xml'>\n"
    . "<!NOTATION gif SYSTEM 'image/gif'>\n<!NOTATION png PUBLIC '-//png' 'png.x'>\n"
    . "<!ENTITY pic SYSTEM 'pic.gif' NDATA gif>\n<!ENTITY pic2 PUBLIC '-//p' 'pic2.gif' NDATA png>\n"
    . "<!-- dtd comment -->\n<?dtdpi x?>\n<!ELEMENT r ANY>\n<!ATTLIST r d CDATA 'dflt'>\n]>\n"
    . "<r a='&e;&amp;'>&e;|&m;|&ext;|</r>";
foreach ([true, false] as $default) {
    echo "=== default handler: ", var_export($default, true), "\n";
    $p = parser($default);
    echo "=> ", xml_parse($p, $doc, true), " ", xml_get_error_code($p), " ", xpos($p), "\n";
}
xml_parse_into_struct(xml_parser_create(), $doc, $v);
print_r($v);

echo "--- external subsets\n";
foreach (["<!DOCTYPE r SYSTEM 'r.dtd'><r/>", "<!DOCTYPE r PUBLIC '-//X' 'r.dtd'><r>&undeclared;</r>"] as $d) {
    $p = parser(true);
    echo "=> ", xml_parse($p, $d, true), " ", xml_get_error_code($p), " ", xpos($p), "\n";
}
