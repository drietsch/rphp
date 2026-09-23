<?php
// ext/xmlwriter: where the output goes — memory, a file (buffered until
// flush), php://output, a stream; the static constructors and the
// procedural functions.
$f = 'xw_test.xml';
$w = new XMLWriter();
var_dump($w->openUri($f));
$w->startElement('file');
$w->text('data');
var_dump(file_get_contents($f));
var_dump($w->flush());
var_dump(file_get_contents($f));
$w->endElement();
var_dump($w->outputMemory());
var_dump($w->flush(false));
var_dump(file_get_contents($f));
unset($w);
unlink($f);

$w = new XMLWriter();
$w->openUri($f);
$w->startDocument();
$w->writeElement('big', str_repeat('x', 5000));
var_dump(strlen(file_get_contents($f)) >= 4000);
$w->endDocument();
var_dump(strlen(file_get_contents($f)));
unset($w);
unlink($f);

$w = new XMLWriter();
$w->openUri('php://output');
$w->startElement('out');
$w->text('put');
$w->endElement();
echo "[before flush]";
var_dump($w->flush());
echo "[after]\n";

$m = XMLWriter::toMemory();
var_dump(get_class($m));
$m->writeElement('tm', 'x');
var_dump($m->outputMemory());

$s = fopen('php://memory', 'w+');
$x = XMLWriter::toStream($s);
$x->writeElement('ts', 'y');
var_dump($x->flush());
rewind($s);
var_dump(stream_get_contents($s));

$u = XMLWriter::toUri($f);
$u->writeElement('tu');
var_dump($u->flush(), file_get_contents($f));
unlink($f);

echo "--- procedural\n";
$w = xmlwriter_open_memory();
var_dump(get_class($w));
xmlwriter_set_indent($w, true);
xmlwriter_set_indent_string($w, '  ');
xmlwriter_start_document($w, '1.0');
xmlwriter_start_element($w, 'r');
xmlwriter_write_attribute($w, 'a', 'b');
xmlwriter_write_element($w, 'c', 'd');
xmlwriter_start_element_ns($w, 'x', 'y', 'urn:x');
xmlwriter_text($w, 'z');
xmlwriter_end_element($w);
xmlwriter_write_comment($w, 'k');
xmlwriter_start_cdata($w);
xmlwriter_text($w, 'cd');
xmlwriter_end_cdata($w);
xmlwriter_write_pi($w, 'p', 'q');
xmlwriter_write_raw($w, '<raw/>');
xmlwriter_full_end_element($w);
xmlwriter_end_document($w);
echo xmlwriter_output_memory($w);
$w = xmlwriter_open_uri($f);
xmlwriter_write_element($w, 'pf', 'v');
var_dump(xmlwriter_flush($w), file_get_contents($f));
unlink($f);
var_dump(new XMLWriter());
