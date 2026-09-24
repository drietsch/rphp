<?php
// spl_autoload() with an explicit extension list: each piece verbatim (a
// space stays part of it), an empty piece tried as the bare name.
@mkdir('spl_tail_ext');
file_put_contents('spl_tail_ext/alpha.inc', "<?php echo \"alpha.inc\\n\"; class Alpha {}\n");
file_put_contents('spl_tail_ext/beta', "<?php echo \"beta (no extension)\\n\"; class Beta {}\n");
file_put_contents('spl_tail_ext/gamma.x', "<?php echo \"gamma.x\\n\";\n");
file_put_contents('spl_tail_ext/gamma.y', "<?php echo \"gamma.y\\n\"; class Gamma {}\n");
set_include_path('spl_tail_ext');

spl_autoload('Alpha', '.x, .inc');
var_dump(class_exists('Alpha', false));
spl_autoload('Alpha', '.inc');
var_dump(class_exists('Alpha', false));
spl_autoload('Beta', ',.php');
var_dump(class_exists('Beta', false));
spl_autoload('Gamma', '.x,.y');
var_dump(class_exists('Gamma', false));
var_dump(array_map('basename', get_included_files()));

foreach (glob('spl_tail_ext/*') as $f) {
    unlink($f);
}
rmdir('spl_tail_ext');
