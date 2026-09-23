<?php
// ext/xmlwriter: failures — false answers, libxml's warnings, php's name
// checks (with their argument numbering), an uninitialized writer.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
$w = new XMLWriter();
t(fn() => $w->startElement('a'));
t(fn() => $w->outputMemory());
t(fn() => $w->flush());
t(fn() => $w->setIndent(true));
$w->openMemory();
t(fn() => $w->writeAttribute('x', 'y'));
t(fn() => $w->endElement());
t(fn() => $w->endAttribute());
t(fn() => $w->startElement('a'));
t(fn() => $w->endAttribute());
t(fn() => $w->writePi('xml', 'x'));
t(fn() => $w->writePi('XmL', 'x'));
t(fn() => $w->writePi('p', 'x?>y'));
t(fn() => $w->startPi('p2'));
t(fn() => $w->startPi('p3'));
t(fn() => $w->startElement('inpi'));
t(fn() => $w->text('in pi'));
t(fn() => $w->endPi());
t(fn() => $w->endPi());
t(fn() => $w->startCdata());
t(fn() => $w->startCdata());
t(fn() => $w->text('a]]>b'));
t(fn() => $w->endCdata());
t(fn() => $w->endCdata());
t(fn() => $w->startComment());
t(fn() => $w->startComment());
t(fn() => $w->text('--'));
t(fn() => $w->endComment());
t(fn() => $w->endComment());
t(fn() => $w->writeComment('a--b'));
t(fn() => $w->startDocument());
t(fn() => $w->fullEndElement());
t(fn() => $w->fullEndElement());
echo $w->outputMemory(), "|\n";

echo "--- name checks\n";
$w = new XMLWriter();
$w->openMemory();
$b = 'b d';
t(fn() => $w->startAttribute($b));
t(fn() => $w->startAttributeNs('p', $b, 'u'));
t(fn() => $w->writeAttribute($b, 'v'));
t(fn() => $w->writeAttributeNs('p', $b, 'u', 'v'));
t(fn() => $w->startElement($b));
t(fn() => $w->startElement(''));
t(fn() => $w->startElement('1a'));
t(fn() => $w->startElementNs('p', $b, 'u'));
t(fn() => $w->writeElement($b));
t(fn() => $w->writeElementNs('p', $b, 'u'));
t(fn() => $w->startPi($b));
t(fn() => $w->writePi($b, 'x'));
t(fn() => $w->startDtdElement($b));
t(fn() => $w->writeDtdElement($b, 'x'));
t(fn() => $w->startDtdAttlist($b));
t(fn() => $w->writeDtdAttlist($b, 'x'));
t(fn() => $w->startDtdEntity($b, false));
t(fn() => $w->writeDtdEntity($b, 'x'));
t(fn() => xmlwriter_start_attribute($w, $b));
t(fn() => xmlwriter_start_attribute_ns($w, 'p', $b, 'u'));
t(fn() => xmlwriter_write_attribute($w, $b, 'v'));
t(fn() => xmlwriter_start_element($w, $b));
t(fn() => xmlwriter_start_element_ns($w, 'p', $b, 'u'));
t(fn() => xmlwriter_write_element($w, $b));
t(fn() => xmlwriter_write_element_ns($w, 'p', $b, 'u'));
t(fn() => xmlwriter_start_pi($w, $b));
t(fn() => xmlwriter_write_pi($w, $b, 'x'));
t(fn() => xmlwriter_write_dtd_element($w, $b, 'x'));
t(fn() => xmlwriter_start_dtd_attlist($w, $b));
t(fn() => xmlwriter_write_dtd_entity($w, $b, 'x'));
t(fn() => xmlwriter_text("x", 'y'));
t(fn() => $w->startElement('a:b'));
t(fn() => $w->startElement("\xc3\xa9"));
t(fn() => $w->startElementNs('bad prefix', 'a', 'urn:x'));
echo $w->outputMemory(), "|\n";

echo "--- URIs\n";
t(fn() => (new XMLWriter)->openUri(''));
t(fn() => (new XMLWriter)->openUri('/nonexistent/dir/x.xml'));
t(fn() => XMLWriter::toUri(''));
t(fn() => xmlwriter_open_uri('/nonexistent/dir/x.xml'));
