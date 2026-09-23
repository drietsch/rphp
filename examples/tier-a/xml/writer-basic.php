<?php
// ext/xmlwriter: libxml2's xmlTextWriter output — when a start tag is
// closed, how text and attribute values are escaped, empty and full end
// tags, comments, PIs, CDATA, raw output, namespaces.
$w = new XMLWriter();
var_dump($w->openMemory());
var_dump($w->startDocument('1.0', 'UTF-8', 'yes'));
var_dump($w->startElement('root'));
var_dump($w->writeAttribute('a', "x<&>\"'\t\n\r\xc3\xa9"));
var_dump($w->startElement('child'));
var_dump($w->text("t<&>\"'\t\n\r]]>\xc3\xa9"));
var_dump($w->endElement());
$w->writeElement('empty');
$w->writeElement('e2', '');
$w->writeElement('e3', 'val');
$w->writeElement('e4', null);
$w->startElement('full');
$w->fullEndElement();
$w->startElement('x');
$w->endElement();
$w->writeComment(' comment ');
$w->writePi('php', 'echo 1;');
$w->writeCdata('cd<>');
$w->writeRaw('<raw/>');
$w->startElement('attrs');
$w->startAttribute('one');
$w->text('v1');
$w->text('<v2>');
$w->endAttribute();
$w->startAttribute('two');
$w->startAttribute('three');
$w->text('3');
$w->startElement('inner');
$w->endElement();
$w->endElement();
$w->startElementNs('p', 'nsel', 'urn:p');
$w->writeAttributeNs('p', 'att', 'urn:p', 'v');
$w->writeAttributeNs('q', 'att2', 'urn:q', 'v2');
var_dump($w->writeAttributeNs('q', 'att3', 'urn:other', 'v3'));
$w->writeAttributeNs(null, 'plain', null, 'pv');
$w->writeElementNs('p', 'inner', 'urn:p', 'iv');
$w->writeElementNs('r', 'inner2', 'urn:r', 'iv2');
$w->writeElementNs(null, 'plain', null, 'pv');
$w->startElementNs(null, 'dflt', 'urn:default');
$w->endElement();
$w->startElementNs('np', 'nouri', null);
$w->endElement();
$w->endElement();
var_dump($w->endElement());
var_dump($w->endElement());
var_dump($w->endDocument());
var_dump($w->outputMemory(false));
var_dump($w->outputMemory());
var_dump($w->outputMemory());

echo "--- endDocument closes what is open\n";
$w = new XMLWriter();
$w->openMemory();
$w->startElement('a');
$w->startElement('b');
$w->writeAttribute('k', 'v');
$w->startComment();
$w->text('still open');
$w->endDocument();
var_dump($w->outputMemory());

echo "--- encodings\n";
foreach (['ISO-8859-1', 'utf-8', 'ascii', 'US-ASCII', 'bogus'] as $enc) {
    $w = new XMLWriter();
    $w->openMemory();
    var_dump($w->startDocument('1.0', $enc));
    $w->writeElement('a', "\xc3\xa9\xe2\x82\xac");
    $w->endDocument();
    echo bin2hex($w->outputMemory()), "\n";
}
$w = new XMLWriter();
$w->openMemory();
var_dump($w->startDocument('2.0', null, 'no'), $w->startDocument(), $w->writeElement('a'), $w->startDocument());
$w->endDocument();
var_dump($w->outputMemory());
