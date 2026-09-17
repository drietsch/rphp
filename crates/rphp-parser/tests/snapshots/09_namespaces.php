<?php
declare(strict_types=1);
namespace A\B;
use C\D, E as F;
use function G\h, I\j as k;
use const L\M;
use N\{O, P as Q, function r, const S};
use function T\{u, v};
const X = 1, Y = 2;
#[Z] const W = 3;
namespace\f(); \g(); h(); A\i();
new namespace\C;
echo __NAMESPACE__, __CLASS__, __FUNCTION__, __METHOD__, __LINE__, __FILE__, __DIR__, __TRAIT__, __PROPERTY__;
namespace Q;
echo 1;
