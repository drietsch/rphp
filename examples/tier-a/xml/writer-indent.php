<?php
// ext/xmlwriter: libxml2's indentation rules — depth before a start tag
// or comment, a newline after an end tag, text turning the next end tag's
// indent off, PIs and CDATA never indented.
$w = new XMLWriter();
$w->openMemory();
$w->setIndent(true);
$w->startDocument();
$w->startElement('a');
$w->writeAttribute('x', '1');
$w->startElement('b');
$w->text('text');
$w->endElement();
$w->startElement('c');
$w->startElement('d');
$w->endElement();
$w->writeElement('e', 'f');
$w->writeComment('cm');
$w->startElement('g');
$w->writeCdata('cdata');
$w->endElement();
$w->startElement('h');
$w->text('mixed');
$w->startElement('i');
$w->endElement();
$w->endElement();
$w->writePi('pi', 'x');
$w->endElement();
$w->endElement();
$w->endDocument();
echo $w->outputMemory();

echo "--- indent string\n";
$w = new XMLWriter();
$w->openMemory();
var_dump($w->setIndentString("\t"), $w->setIndent(1));
$w->startElement('a');
$w->startElement('b');
$w->writeElement('c');
$w->endElement();
$w->endElement();
echo $w->outputMemory(), "|\n";

echo "--- raw output and switching indentation off\n";
$w = new XMLWriter();
$w->openMemory();
$w->setIndent(true);
$w->setIndentString('..');
$w->startElement('a');
$w->startElement('b');
$w->writeRaw('raw');
$w->startElement('c');
$w->endElement();
$w->endElement();
$w->writeElement('d', 'x');
$w->setIndent(false);
$w->writeElement('e', 'y');
$w->endElement();
echo $w->outputMemory(), "|\n";

echo "--- an open attribute, comments, CDATA and PIs inside text\n";
$w = new XMLWriter();
$w->openMemory();
$w->setIndent(true);
$w->startElement('a');
$w->startAttribute('x');
$w->startElement('b');
$w->text('t');
$w->startComment();
$w->text('c');
$w->endComment();
$w->startCdata();
$w->text('d');
$w->endCdata();
$w->writePi('p', 'q');
$w->endElement();
$w->fullEndElement();
$w->startElement('z');
$w->fullEndElement();
echo $w->outputMemory(), "|\n";

echo "--- a DTD, indented and not\n";
foreach ([false, true] as $indent) {
    $w = new XMLWriter();
    $w->openMemory();
    $w->setIndent($indent);
    $w->startDocument();
    $w->startDtd('html', '-//W3C//DTD XHTML 1.0 Strict//EN', 'http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd');
    $w->writeDtdElement('p', '(#PCDATA)');
    $w->writeDtdAttlist('p', 'id ID #IMPLIED');
    $w->writeDtdEntity('ent', 'value', false);
    $w->endDtd();
    $w->writeDtd('d2', null, 'sys');
    $w->startElement('html');
    $w->writeElement('body', 'x');
    $w->endElement();
    $w->endDocument();
    echo $w->outputMemory(), "|\n";
}
