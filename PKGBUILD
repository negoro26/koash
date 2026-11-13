pkgname=koash
pkgver=0.2.0
pkgrel=1
pkgdesc='Agent-native structured command shell written in Rust (koa)'
arch=('x86_64' 'aarch64')
url='https://github.com/negoro26/koash'
license=('MIT')
depends=('gcc-libs' 'glibc')
makedepends=('cargo')
provides=('koa')
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$pkgname-$pkgver"
  cargo build --release --locked --bin koa
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 target/release/koa "$pkgdir/usr/bin/koa"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
