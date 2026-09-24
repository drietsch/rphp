<?php
// spl_autoload(): php's default loader — lowercased class name plus each of
// spl_autoload_extensions() looked up on the include path, included once.
@mkdir('spl_tail_lib');
@mkdir('spl_tail_lib/my');
file_put_contents('spl_tail_lib/foo.inc', "<?php echo \"loaded foo.inc\\n\"; class Foo {}\n");
file_put_contents('spl_tail_lib/bar.php', "<?php echo \"loaded bar.php\\n\"; class Bar {}\n");
file_put_contents('spl_tail_lib/my/baz.php', "<?php namespace My; echo \"loaded my/baz.php\\n\"; class Baz {}\n");
file_put_contents('spl_tail_lib/qux.php', "<?php echo \"loaded qux.php (declares nothing)\\n\";\n");

var_dump(spl_autoload_extensions());
set_include_path('spl_tail_lib');

// Registering nothing registers spl_autoload itself.
var_dump(spl_autoload_register());
var_dump(spl_autoload_functions());
var_dump(class_exists('Foo'), class_exists('BAR'), class_exists('My\Baz'), class_exists('Nope'));

// Straight calls: include-once semantics, then the class check.
spl_autoload('Qux');
spl_autoload('Qux');
spl_autoload('foo');
spl_autoload('../spl_tail_lib/bar');
spl_autoload("Bad\0Name");

var_dump(spl_autoload_extensions('.php'));
var_dump(spl_autoload_extensions('.inc, .php'));
var_dump(spl_autoload_extensions(null));
var_dump(spl_autoload_extensions(''));
var_dump(spl_autoload_extensions('.inc,.php'));

var_dump(spl_autoload_unregister('spl_autoload'));
var_dump(spl_autoload_functions());
var_dump(spl_autoload_register(null, false));
var_dump(spl_autoload_register('spl_autoload'));
var_dump(count(spl_autoload_functions()));

foreach (['spl_tail_lib/foo.inc', 'spl_tail_lib/bar.php', 'spl_tail_lib/my/baz.php', 'spl_tail_lib/qux.php'] as $f) {
    unlink($f);
}
rmdir('spl_tail_lib/my');
rmdir('spl_tail_lib');
