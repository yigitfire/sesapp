// Masaüstü uygulamasında mikrofon sesi Rust'tan 10 ms'lik paketlerle gelir
// (480 örnek, 48 kHz, 16-bit). Bu işlemci paketleri küçük bir tamponda biriktirip
// Web Audio'ya 128 örneklik bloklar halinde verir.
//
// Mikrofonun saati ile ses kartının saati birebir aynı değildir (çok küçük kayma olur).
// Tampon hedeften uzaklaşınca blok başına tek bir örnek atlanır/tekrarlanır; bu duyulmaz
// ve gecikmenin zamanla büyümesini (veya tamponun boşalmasını) engeller.

const HEDEF = 960      // 20 ms: paketlerin düzensiz gelmesine karşı pay
const ALT = 480        // 10 ms altına düşerse bir örnek tekrarla
const UST = 1920       // 40 ms üstüne çıkarsa bir örnek atla
const TAVAN = 4800     // 100 ms: bir şey takıldıysa fazlayı at, gecikmeyi sıfırla
const BOYUT = 16384

class MasaustuMikrofon extends AudioWorkletProcessor {
  constructor() {
    super()
    this.tampon = new Float32Array(BOYUT)
    this.oku = 0
    this.yaz = 0
    this.dolu = 0
    this.calisiyor = false
    this.port.onmessage = e => this.ekle(new Int16Array(e.data))
  }

  ekle(pcm) {
    for (let i = 0; i < pcm.length; i++) {
      this.tampon[this.yaz] = pcm[i] / 32768
      this.yaz = (this.yaz + 1) % BOYUT
    }
    this.dolu += pcm.length
    if (this.dolu > TAVAN) {
      const at = this.dolu - HEDEF
      this.oku = (this.oku + at) % BOYUT
      this.dolu = HEDEF
    }
  }

  process(_girisler, cikislar) {
    const cikis = cikislar[0][0]
    const n = cikis.length

    if (!this.calisiyor) {
      if (this.dolu < HEDEF) { cikis.fill(0); return true }
      this.calisiyor = true
    }
    if (this.dolu < n) {
      // Tampon boşaldı: sessizlik ver, yeniden HEDEF kadar dolmasını bekle.
      cikis.fill(0)
      this.calisiyor = false
      return true
    }

    // Kaymayı düzelt: tüketilecek örnek sayısı n-1, n veya n+1.
    let tuket = n
    if (this.dolu > UST) tuket = n + 1
    else if (this.dolu < ALT) tuket = n - 1
    const oran = (tuket - 1) / (n - 1)
    for (let i = 0; i < n; i++) {
      const konum = i * oran
      const a = Math.floor(konum)
      const k = konum - a
      const s0 = this.tampon[(this.oku + a) % BOYUT]
      const s1 = this.tampon[(this.oku + Math.min(a + 1, tuket - 1)) % BOYUT]
      cikis[i] = s0 + (s1 - s0) * k
    }
    this.oku = (this.oku + tuket) % BOYUT
    this.dolu -= tuket
    return true
  }
}

registerProcessor('masaustu-mikrofon', MasaustuMikrofon)
