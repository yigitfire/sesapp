// Ses kapısı (hassasiyet): ses, eşikten (dB) yüksekse geçer, alçaksa kısılır.
// Yankı ve arka plan sesi genelde konuşmadan kısık olduğu için kişi konuşmuyorken kesilir.
// Ses kartının iş parçacığında çalışır: pencere arkadayken/oyun açıkken de yavaşlamaz.
//
// esik: dB (-100 = kapı kapalı, ses hep geçer). Ses eşiği geçince hemen açılır (2 ms),
// eşiğin altına düşünce 250 ms daha açık kalır (kelime aralarında kesilmesin), sonra
// 60 ms'de yumuşakça kapanır (tık sesi olmasın).

const BEKLE_SN = 0.25

class SesKapisi extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    return [{name: 'esik', defaultValue: -100, minValue: -100, maxValue: 0, automationRate: 'k-rate'}]
  }

  constructor() {
    super()
    this.kazanc = 1
    this.acikKalan = 0
    this.ac = 1 - Math.exp(-1 / (0.002 * sampleRate))
    this.kapa = 1 - Math.exp(-1 / (0.06 * sampleRate))
  }

  process(girisler, cikislar, ayar) {
    const giris = girisler[0], cikis = cikislar[0]
    const esik = ayar.esik[0]
    if (!giris || !giris.length) {
      for (const k of cikis) k.fill(0)
      return true
    }
    const n = giris[0].length
    let hedef = 1
    if (esik > -99.5) {
      // Bloğun (128 örnek ≈ 2,7 ms) gücü; kanallardan en yükseği.
      let guc = 0
      for (const k of giris) {
        let g = 0
        for (let i = 0; i < n; i++) g += k[i] * k[i]
        guc = Math.max(guc, g / n)
      }
      const db = 10 * Math.log10(guc + 1e-12)
      if (db > esik) this.acikKalan = BEKLE_SN * sampleRate
      else this.acikKalan -= n
      hedef = this.acikKalan > 0 ? 1 : 0
    }
    const hiz = hedef > this.kazanc ? this.ac : this.kapa
    for (let c = 0; c < cikis.length; c++) {
      const g = giris[Math.min(c, giris.length - 1)], o = cikis[c]
      let kazanc = this.kazanc
      for (let i = 0; i < n; i++) {
        kazanc += (hedef - kazanc) * hiz
        o[i] = g[i] * kazanc
      }
      if (c === cikis.length - 1) this.kazanc = kazanc
    }
    return true
  }
}

registerProcessor('ses-kapisi', SesKapisi)

// Yankı bastırma (hoparlörle konuşanlar için). İki giriş: 0 = mikrofon, 1 = hoparlörden çalan
// ses (başkalarının sesi). Başkaları konuşurken mikrofona gelen ses, sadece yankı
// olabilecek seviyeden açıkça yüksekse (kişi kendisi de konuşuyorsa) geçer; değilse kısılır.
//
// - Hoparlör seviyesi 400 ms tutulur, sonra iner: yankı mikrofona hoparlörden biraz geç ulaşır.
// - Yankının ne kadar yüksek geldiği (mikrofon - hoparlör farkı, dB) kendiliğinden öğrenilir:
//   başkaları en az 400 ms konuştuktan sonra, farkın çoğu zaman (%75) altında kaldığı seviye
//   izlenir. Kişi bunun 6 dB üstündeyse konuşuyor sayılır.
// - Kendi sesi geçince 250 ms açık kalır; kısma yumuşaktır. Tam susturmaz, -30 dB.

const SESSIZ_DB = -55, PAY_DB = 6, KISIK = 0.03, TUT_SN = 0.4, INIS_DB_SN = 60

class YankiBastirici extends AudioWorkletProcessor {
  static get parameterDescriptors() {
    return [{name: 'acik', defaultValue: 0, minValue: 0, maxValue: 1, automationRate: 'k-rate'}]
  }

  constructor() {
    super()
    this.kazanc = 1
    this.hoparlor = -120   // hoparlör seviyesi (dB), 400 ms tutulur
    this.tutKalan = 0
    this.mikrofon = -120   // mikrofon seviyesi (dB), hemen çıkar, 60 dB/sn iner
    this.hoparlorSure = 0  // başkaları ne zamandır konuşuyor (sn)
    this.kayip = 6         // öğrenilen yankı farkı (dB); yüksekten başlar: emin olana kadar çok kısar
    this.konusuyorKalan = 0
    this.ac = 1 - Math.exp(-1 / (0.003 * sampleRate))
    this.kapa = 1 - Math.exp(-1 / (0.04 * sampleRate))
  }

  static db(kanallar, n) {
    let guc = 0
    for (const k of kanallar || []) {
      let g = 0
      for (let i = 0; i < n; i++) g += k[i] * k[i]
      guc = Math.max(guc, g / n)
    }
    return 10 * Math.log10(guc + 1e-12)
  }

  process(girisler, cikislar, ayar) {
    const mik = girisler[0], cikis = cikislar[0]
    if (!mik || !mik.length) {
      for (const k of cikis) k.fill(0)
      return true
    }
    const n = mik[0].length, dt = n / sampleRate
    let hedef = 1
    if (ayar.acik[0] >= 0.5) {
      const mikDb = YankiBastirici.db(mik, n), hopDb = YankiBastirici.db(girisler[1], n)
      if (hopDb >= this.hoparlor) { this.hoparlor = hopDb; this.tutKalan = TUT_SN }
      else if ((this.tutKalan -= dt) <= 0) this.hoparlor = Math.max(hopDb, this.hoparlor - INIS_DB_SN * dt)
      this.mikrofon = Math.max(mikDb, this.mikrofon - INIS_DB_SN * dt)
      if (this.hoparlor > SESSIZ_DB) {
        this.hoparlorSure += dt
        const fark = this.mikrofon - this.hoparlor
        const konusuyor = fark > this.kayip + PAY_DB
        // Öğrenme: fark seviyenin altındaysa 6 dB/sn iner, biraz üstündeyse 18 dB/sn çıkar
        // (= farkın %75'i bu seviyenin altında). Kişi konuşuyor görünüyorsa sadece 2 dB/sn
        // çıkar: hem aynı anda konuşurken yanlış öğrenmez, hem de seviye fazla düşük kaldıysa
        // yavaşça düzelir.
        if (this.hoparlorSure > TUT_SN && this.mikrofon > -70) {
          const hiz = fark < this.kayip ? -6 : konusuyor ? 2 : 18
          this.kayip = Math.max(-40, Math.min(20, this.kayip + hiz * dt))
        }
        if (konusuyor) this.konusuyorKalan = 0.25
        else this.konusuyorKalan -= dt
        hedef = this.konusuyorKalan > 0 ? 1 : KISIK
      } else { this.hoparlorSure = 0; this.konusuyorKalan = 0 }
    }
    const hiz = hedef > this.kazanc ? this.ac : this.kapa
    for (let c = 0; c < cikis.length; c++) {
      const g = mik[Math.min(c, mik.length - 1)], o = cikis[c]
      let kazanc = this.kazanc
      for (let i = 0; i < n; i++) {
        kazanc += (hedef - kazanc) * hiz
        o[i] = g[i] * kazanc
      }
      if (c === cikis.length - 1) this.kazanc = kazanc
    }
    return true
  }
}

registerProcessor('yanki-bastirici', YankiBastirici)
