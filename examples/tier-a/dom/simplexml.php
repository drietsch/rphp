<?php
// SimpleXMLElement: the iterator kinds behind `$x->name`, `[i]`, `[attr]`,
// `children()` and `attributes()`; string/number/bool casts; counting and
// iteration; xpath with registered and in-scope namespaces; writes
// (`$x->new = ...`, `$x->a[0]["at"] = ...`, `unset`); `asXML()` of the
// document, an element and an attribute; the property table `var_dump`,
// `(array)` and `json_encode` see; subclasses; DOM interop.
$xml = '<?xml version="1.0"?>
<root xmlns:p="urn:p" id="r1"><item n="1">one</item><item n="2"><sub>deep</sub>tail</item><p:x a="b">px</p:x><empty/><item n="3"/><!-- c --></root>';
$s = simplexml_load_string($xml);
var_dump(get_class($s), $s->getName(), (string)$s->item, (string)$s->item[1]->sub, count($s->item), isset($s->item), isset($s->nope), isset($s['id']), (string)$s['id'], (string)$s->item[0]['n'], $s->item[5], $s->nope);
foreach ($s->item as $i => $it) { echo $i, "=", (string)$it, " n=", $it['n'], "\n"; }
foreach ($s as $k => $v) { echo $k, ":", trim((string)$v), "|"; } echo "\n";
foreach ($s->attributes() as $k => $v) { echo "attr $k=$v\n"; }
foreach ($s->item[1]->attributes() as $k => $v) { echo "attr $k=$v\n"; }
var_dump(count($s), count($s->children()), count($s->item[1]), count($s->item[1]->children()), (string)$s->item[1], $s->item[1]->sub->getName());
var_dump($s->xpath('//item[@n="2"]/sub')[0]->getName(), (string)$s->xpath('//item')[2]['n'], $s->xpath('//nope'), $s->item[0]->xpath('..')[0]->getName());
$s->registerXPathNamespace('q', 'urn:p'); var_dump((string)$s->xpath('//q:x')[0], (string)$s->xpath('//q:x/@a')[0]);
var_dump($s->children('urn:p')->x->getName(), (string)$s->children('urn:p')->x['a'], (string)$s->children('p', true)->x, count($s->children('urn:p')));
var_dump($s->getNamespaces(), $s->getNamespaces(true), $s->getDocNamespaces());
$c = $s->addChild('added', 'val < & x'); $c->addAttribute('k', 'v'); $c->addChild('inner'); $s->addChild('p:pc', 'pc', 'urn:p');
echo $s->asXML(), "\n";
echo $s->item[1]->asXML(), "\n", $s->item[0]['n']->asXML(), "\n";
$s->item[0] = 'replaced'; $s->item[0]['n'] = '11'; $s['id'] = 'r2'; $s->newprop = 'np'; $s->newprop2['at'] = 'x';
unset($s->empty); unset($s->item[2]); unset($s['nope']);
echo $s->asXML(), "\n";
var_dump((array)$s->item[1], json_encode($s), json_encode($s->item[0]), (bool)$s->empty, (bool)$s->item, (int)$s->item[0]['n'], $s->item[0]['n'] == 11, "11" == $s->item[0]['n']);
var_dump($s);
print_r($s->item[1]);
var_dump(simplexml_load_string('<a>x</a>')->asXML(), simplexml_load_string('<a><b>1</b><b>2</b></a>')->b[1] == '2', (string)simplexml_load_string('<a>t<b/>u</a>'), trim((string)simplexml_load_string('<a> t </a>')));
$d = new DOMDocument; $d->loadXML('<r><a>1</a></r>'); $sx = simplexml_import_dom($d); var_dump((string)$sx->a, get_class($sx)); $de = dom_import_simplexml($sx->a); var_dump(get_class($de), $de->textContent, $de->ownerDocument === $d);
class My extends SimpleXMLElement { function hi() { return 'hi ' . $this->getName(); } }
$m = simplexml_load_string('<r><a/></r>', 'My'); var_dump(get_class($m), $m->hi(), get_class($m->a), $m->a->hi());
$n = new SimpleXMLElement('<q><w>1</w></q>'); var_dump((string)$n->w, $n instanceof Traversable, $n instanceof Countable, $n instanceof Stringable);
libxml_use_internal_errors(true); var_dump(simplexml_load_string('<bad>'), count(libxml_get_errors())); libxml_clear_errors();
try { new SimpleXMLElement('<bad>'); } catch (Exception $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(simplexml_load_string('<a><b>1</b></a>')->b->children(), (string)simplexml_load_string('<a><b>1</b></a>')->c, simplexml_load_string('<a/>')->children()->count());
$it = simplexml_load_string('<a><b>1</b><b>2</b><c/></a>'); var_dump($it->count(), iterator_count($it), iterator_count($it->b));
foreach ($it->children() as $name => $node) { echo $name, ";"; } echo "\n";
var_dump(empty($it->b), empty($it->zz), isset($it->b[1]), isset($it->b[5]), $it->b[1] instanceof SimpleXMLElement, (string)$it->b[1]);

$s = simplexml_load_string("<a x=\"1\"><b y=\"2\">t</b><b>u</b><c/></a>"); var_dump($s->children(), $s->attributes(), $s->b, $s->b[0]->children(), $s->b[1], $s->c, $s->c->children(), (array)$s->b[0], (array)$s, (array)$s->attributes()); var_dump(json_encode($s->attributes()), json_encode($s->b), json_encode($s->b[0]), json_encode($s->c), json_encode($s->children()));
print_r($s); print_r($s->b);
var_dump(get_object_vars($s), iterator_to_array($s->b, false)[1] == 'u');
foreach ($s->attributes() as $k => $v) { var_dump($k, (string)$v, $v->getName()); }
