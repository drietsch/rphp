<?php
// libxml's error sequences for malformed input: codes, levels, lines,
// columns and messages.
libxml_use_internal_errors(true); foreach (["<r><a>", "<r><a></b></r>", "<r><a></r><b>", "<r>x</r><s/>", "<r attr=1/>", "<r><a b=\"1\" b=\"2\"/></r>", "<r>&#0;</r>", "<r>a]]>b</r>", "<r><!-- x -- y --></r>", "<r>&bad;</r>", "<r a=\"<\"/>", "<r xmlns:p=\"u\"><q:a/></r>", "not xml", "   ", "<?xml version=\"1.0\"?>\n<r>\n  <a>\n</r>", "<r><a x='1' y='2'>t</a><b/></r>"] as $x) { $d = new DOMDocument; var_dump($d->loadXML($x)); foreach (libxml_get_errors() as $e) echo "  ", $e->level, " ", $e->code, " L", $e->line, " C", $e->column, " ", $e->message; libxml_clear_errors(); }
