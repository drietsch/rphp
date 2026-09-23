<?php
// ext/xmlreader: parse errors (the first fatal one fails the first read),
// namespace errors (reported, reading goes on), libxml's internal error
// list, the API's error paths, reading files and streams.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
libxml_use_internal_errors(true);
foreach ([
    "<a><b>1</b><c></a>",
    "<r>\n<a>\n</b></r>",
    "<r>&undef;</r>",
    "junk",
    "<r><p:a/></r>",
    "<r a='1' a='2'/>",
    "<r><a></r>",
] as $x) {
    $r = XMLReader::fromString($x);
    echo json_encode($x), ":";
    while ($r->read()) {
        echo " ", $r->nodeType, $r->name;
    }
    echo "\n";
    foreach (libxml_get_errors() as $e) {
        echo "  ", $e->level, " ", $e->code, " ", $e->line, " ", $e->column, " ", trim($e->message), "\n";
    }
    libxml_clear_errors();
}
libxml_use_internal_errors(false);

echo "--- the API\n";
$r = new XMLReader();
t(fn() => $r->read());
t(fn() => $r->getAttribute('x'));
t(fn() => $r->moveToFirstAttribute());
t(fn() => $r->moveToElement());
t(fn() => $r->readOuterXml());
t(fn() => $r->readInnerXml());
t(fn() => $r->readString());
t(fn() => $r->expand());
t(fn() => $r->setParserProperty(XMLReader::LOADDTD, true));
t(fn() => $r->close());
t(fn() => [$r->name, $r->nodeType, $r->depth, $r->isEmptyElement, $r->baseURI]);
t(fn() => $r->name = 'x');
t(fn() => $r->XML(''));
t(fn() => $r->open(''));
t(fn() => $r->open('/nonexistent/file.xml'));
t(fn() => XMLReader::fromString(''));
t(fn() => XMLReader::fromUri(''));
$r = XMLReader::fromString('<a x="1"/>');
t(fn() => [$r->name, $r->nodeType, $r->attributeCount]);
t(fn() => $r->isEmptyElement);
t(fn() => $r->getParserProperty(XMLReader::LOADDTD));
t(fn() => $r->getParserProperty(99));
t(fn() => $r->setParserProperty(99, true));
t(fn() => $r->isValid());
t(fn() => $r->getAttribute(''));
t(fn() => $r->moveToAttribute(''));
t(fn() => $r->getAttributeNs('', 'u'));
t(fn() => $r->getAttributeNs('a', ''));
t(fn() => $r->lookupNamespace(''));
$r->read();
t(fn() => $r->isEmptyElement);
t(fn() => $r->read());
t(fn() => $r->close());
t(fn() => $r->read());

echo "--- files and streams\n";
$f = 'xr_test.xml';
file_put_contents($f, "<doc>\n  <x a='1'>1</x>\n</doc>");
$r = new XMLReader();
var_dump($r->open($f));
while ($r->read()) {
    echo $r->nodeType, $r->name, $r->depth, " ";
}
echo "\n";
$r = XMLReader::fromUri($f);
$r->read();
var_dump($r->readOuterXml());
$s = fopen($f, 'r');
$r = XMLReader::fromStream($s);
$r->read();
$r->read();
$r->read();
var_dump($r->name, $r->getAttribute('a'));
unlink($f);
