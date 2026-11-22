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
