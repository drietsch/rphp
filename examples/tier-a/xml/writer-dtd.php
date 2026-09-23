<?php
// ext/xmlwriter: DOCTYPE declarations and the internal subset — element,
// attribute-list and entity declarations, started and ended or written
// whole, and what libxml refuses.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
$w = new XMLWriter();
$w->openMemory();
$w->startDocument();
t(fn() => $w->startDtd('html', '-//W3C//DTD XHTML 1.0 Strict//EN', 'http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd'));
t(fn() => $w->writeDtdElement('p', '(#PCDATA)'));
t(fn() => $w->startDtdElement('q'));
t(fn() => $w->text('EMPTY'));
t(fn() => $w->endDtdElement());
t(fn() => $w->writeDtdAttlist('p', 'id ID #IMPLIED'));
t(fn() => $w->startDtdAttlist('q'));
t(fn() => $w->text('x CDATA "d"'));
t(fn() => $w->endDtdAttlist());
t(fn() => $w->writeDtdEntity('ent', 'value', false));
t(fn() => $w->writeDtdEntity('pent', 'pv', true));
t(fn() => $w->writeDtdEntity('ext', '', false, '-//P', 'sys.xml'));
t(fn() => $w->writeDtdEntity('ext2', '', false, null, 'sys2.xml', 'gif'));
t(fn() => $w->writeDtdEntity('bad', '', true, null, 'sys3.xml', 'gif'));
t(fn() => $w->startDtdEntity('se', false));
t(fn() => $w->text('sev'));
t(fn() => $w->endDtdEntity());
t(fn() => $w->writeComment('in dtd'));
t(fn() => $w->writePi('pi', 'dtd'));
t(fn() => $w->endDtdEntity());
t(fn() => $w->endDtd());
t(fn() => $w->endDtd());
t(fn() => $w->writeDtd('d2', null, 'sys'));
t(fn() => $w->writeDtd('d3'));
t(fn() => $w->writeDtd('d4', 'pub'));
t(fn() => $w->writeDtd('d5', null, null, '<!ENTITY x "y">'));
t(fn() => $w->startElement('html'));
t(fn() => $w->endElement());
t(fn() => $w->endDocument());
echo $w->outputMemory(), "|\n";

echo "--- a subset written whole\n";
$w = new XMLWriter();
$w->openMemory();
t(fn() => $w->writeDtd('r', null, null, '<!ELEMENT r ANY>'));
t(fn() => $w->writeDtd('s', 'pub', 'sys', '<!ELEMENT s EMPTY>'));
t(fn() => $w->writeDtdElement('late', 'ANY'));
echo $w->outputMemory(), "|\n";

echo "--- declarations outside a DTD\n";
$w = new XMLWriter();
$w->openMemory();
t(fn() => $w->writeDtdElement('e', 'ANY'));
t(fn() => $w->writeDtdAttlist('e', 'a CDATA #IMPLIED'));
t(fn() => $w->writeDtdEntity('e', 'v'));
t(fn() => $w->startElement('a'));
t(fn() => $w->writeDtdElement('e', 'ANY'));
t(fn() => $w->startDtd('x'));
echo $w->outputMemory(), "|\n";
