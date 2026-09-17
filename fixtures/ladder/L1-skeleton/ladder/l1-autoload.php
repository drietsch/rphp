<?php
// Ladder rung L1: Composer's autoloader + platform check under rphp.
// Expected to be byte-identical between `php` and `rphp`.

require dirname(__DIR__) . '/vendor/autoload.php';

var_dump(class_exists(Symfony\Component\Console\Application::class));
var_dump(interface_exists(Psr\Container\ContainerInterface::class));
echo \Composer\InstalledVersions::getVersion('symfony/console'), "\n";
echo \Composer\InstalledVersions::isInstalled('symfony/framework-bundle') ? "fb:yes\n" : "fb:no\n";
echo PHP_VERSION_ID >= 80401 ? "php-ok\n" : "php-old\n";
echo PHP_INT_SIZE, ' ', PHP_EOL === "\n" ? 'eol-lf' : 'eol-other', "\n";
$fn = 'Symfony\\Component\\Console\\Application';
var_dump((new ReflectionClass($fn))->getShortName());
echo count(spl_autoload_functions()) > 0 ? "autoloaders:registered\n" : "autoloaders:none\n";
