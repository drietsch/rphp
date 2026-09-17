<?php
// Ladder rung L1 (2/2): spl_autoload_register() ordering around Composer's
// loader, class_exists() across packages, Composer\InstalledVersions.
// Deterministic output; expected to be byte-identical between `php` and `rphp`.

$log = [];
spl_autoload_register(static function (string $class) use (&$log): void {
    $log[] = "appended:$class";
});

/** @var \Composer\Autoload\ClassLoader $loader */
$loader = require dirname(__DIR__) . '/vendor/autoload.php';

spl_autoload_register(static function (string $class) use (&$log): void {
    $log[] = "prepended:$class";
}, true, true);

echo 'autoloaders: ', count(spl_autoload_functions()), "\n";
var_dump($loader instanceof \Composer\Autoload\ClassLoader);

// Symbols from packages other than symfony/console: symfony/yaml, psr/log,
// symfony/service-contracts.
var_dump(class_exists(Symfony\Component\Yaml\Yaml::class));
var_dump(interface_exists(Psr\Log\LoggerInterface::class));
var_dump(trait_exists(Psr\Log\LoggerAwareTrait::class));
var_dump(interface_exists(Symfony\Contracts\Service\ResetInterface::class));
var_dump(class_exists('Nope\\Missing\\Thing'));
var_dump(class_exists('Nope\\Missing\\Thing', false));

// Composer registers its loader with prepend=true, so it runs before the
// closure registered first; the second closure was prepended after it.
foreach ($log as $line) {
    echo $line, "\n";
}

$root = dirname(__DIR__);
echo substr($loader->findFile(Symfony\Component\Yaml\Yaml::class), strlen($root)), "\n";
var_dump($loader->findFile('Nope\\Missing\\Thing'));
echo count($loader->getPrefixesPsr4()) > 10 ? "psr4:many\n" : "psr4:few\n";
var_dump(isset($loader->getPrefixesPsr4()['App\\']));

$packages = \Composer\InstalledVersions::getInstalledPackages();
sort($packages);
echo 'packages: ', count($packages), "\n";
echo 'first: ', $packages[0], "\n";
echo 'last: ', $packages[count($packages) - 1], "\n";
echo 'root: ', \Composer\InstalledVersions::getRootPackage()['name'], "\n";
var_dump(\Composer\InstalledVersions::isInstalled('symfony/yaml'));
var_dump(\Composer\InstalledVersions::isInstalled('nope/nope'));
echo \Composer\InstalledVersions::getPrettyVersion('symfony/yaml'), "\n";
echo count(\Composer\InstalledVersions::getInstalledPackagesByType('symfony-bundle')), " bundle(s)\n";
