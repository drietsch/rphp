<?php
// The class-inspection natives take a name through the autoloader, as
// php's `zend_lookup_class` does: `method_exists`, `property_exists`,
// `get_class_methods`, `get_parent_class`, `get_class_vars` always, the
// `class_*` family per its `$autoload` argument (its warning changes with
// it). An unknown name is a `TypeError` where php has one.
spl_autoload_register(function ($c) { echo "autoload($c)\n"; if ($c === 'Late') { eval('class Late { public $p = 1; function m() {} }'); } });
var_dump(method_exists('Late', 'm'));
var_dump(property_exists('Late2', 'p'));
try { var_dump(get_class_methods('Late3')); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { var_dump(get_parent_class('Late4')); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
try { var_dump(get_class_vars('Late11')); } catch (TypeError $e) { echo $e->getMessage(), "\n"; }
var_dump(get_class_vars('Late'));
var_dump(class_implements('Late5'));
var_dump(class_implements('Late6', false));
var_dump(class_uses('Late7'));
var_dump(class_parents('Late8', false));
var_dump(is_subclass_of('Late', 'Late9'));
var_dump(is_a('Late', 'Late10', true));

// An empty name, or one with characters a class name cannot hold, never
// reaches the loaders; the leading backslash is stripped after that check.
var_dump(class_exists(''), method_exists('', 'x'), is_a('', 'x', true), class_exists('\\'), class_exists('\\Foo'), class_exists('Foo\\'), class_exists('a b'));
