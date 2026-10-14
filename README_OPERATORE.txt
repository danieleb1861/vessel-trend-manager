VESSEL TREND MANAGER - GUIDA PER L'OPERATORE

Consulta i trend dei macchinari dagli export .s3db del sistema AMS, su un PC
separato. I dati arrivano fino all'ultimo export disponibile: non e un
collegamento in tempo reale. Il programma non modifica i database.

PRIMO AVVIO
1. Copiare lo ZIP sul PC e scegliere Estrai tutto.
2. Aprire la cartella VesselTrendManager e avviare VesselTrendManager.exe.
   Conservare la guida e i file di licenza insieme al programma.
3. Premere Cartella dati... e scegliere EXPORT, con days e weeks, oppure
   direttamente la cartella contenente i file .s3db.
4. Attendere Archivio pronto. In alto sono visibili le date disponibili
   per ciascuna sorgente. Il pacchetto e previsto per Windows 10/11 x64;
   non richiede installazioni aggiuntive o Internet sul PC dell'operatore.

CARICARE UN TREND
1. Scegliere l'impianto a sinistra, oppure Tutti gli impianti.
2. Cercare parte del nome, della descrizione o del tag del parametro.
   Per includere gli stati di marcia scegliere Digitali o Tutti.
3. Spuntare i parametri: si possono confrontare fino a quattro segnali.
   Per toglierne uno usare Rimuovi oppure togliere la spunta.
4. Scegliere Giornalieri per le letture originali oppure Settimanali per
   gli aggregati e i relativi minimi/massimi. Sono sorgenti diverse.
5. Scegliere il periodo dal menu: 24 ore, 7, 30, 90, 180 o 365 giorni,
   oppure Tutto lo storico. Per un periodo preciso scrivere Da e A.
6. Premere Carica trend. Se necessario usare Annulla caricamento in basso.
   I grafici mostrano il periodo effettivamente caricato sopra i pannelli.
   Dopo una modifica alla selezione premere nuovamente Carica trend.

VEDERE PERIODI PIU VECCHI O PIU LUNGHI
Non c'e un limite fisso di 30 giorni. Si possono inserire date passate
oppure usare Tutto lo storico della sorgente selezionata.
I periodi rapidi terminano all'ultima lettura disponibile, non all'ora del PC.
Se esistono solo export settimanali per un periodo vecchio, scegliere
Settimanali: il programma non ricrea letture giornaliere mancanti.

Esempio per tutta una giornata:
  Da: 2025-01-10 00:00
  A:  2025-01-11 00:00
Il formato e AAAA-MM-GG HH:MM e l'istante A e escluso.
Gli orari sono quelli dell'archivio, senza conversione di fuso.

Una richiesta molto grande puo essere interrotta dal limite di tempo o
di campioni. Ridurre parametri o periodo, oppure usare i settimanali per
una vista d'insieme. Non viene mostrato un risultato parziale come completo.

LEGGERE IL GRAFICO
- Controllare data e ora dell'ultima lettura sopra ciascun grafico.
  Un dato vecchio non e una misura attuale; nessun dato non significa zero.
- Passare il mouse sul grafico per vedere nel riquadro valore, unita,
  timestamp e file di origine del campione conservato piu vicino.
- Usare la rotella per lo zoom e trascinare con il pulsante sinistro per
  spostarsi. Ripristina zoom ripristina il periodo. Con il pulsante destro
  trascinare un rettangolo per ingrandire una zona.
- Salva schermata salva un'immagine PNG della finestra visibile. La schermata
  puo contenere nomi di impianti, letture e percorsi: conservarla localmente.
  Scegliere un nome nuovo con estensione .png: i file esistenti non vengono sovrascritti.
- Per piu dettaglio, restringere Da/A e premere nuovamente Carica trend.
  Il solo zoom non carica nuovi campioni. Il grafico conserva una selezione
  di letture valide, inclusi gli estremi, per mantenere la finestra reattiva.
- Tacche arancio: letture non valide o forzate, escluse dalla curva.
- Tacche rosse: flag di allarme gia registrati dal sistema AMS.
- Tacche viola: flag di inibizione gia registrati.
- Fasce grigie: archivi mancanti per quella parte del periodo.
- Record precedente: lettura anteriore al periodo, che non conferma lo
  stato durante il periodo. Le interruzioni della curva non provano un guasto.
Le tacche non rappresentano la durata esatta di un allarme.

LIMITI DEI MACCHINARI
Limiti macchinari... importa un file CSV con una riga per parametro:
  tag;min;max;unita;note
Il file si apre e si modifica direttamente con Excel: salvare con Salva,
mantenendo il formato CSV. Dopo il salvataggio i grafici si aggiornano da soli.
Il tag e il nome esatto del parametro nell'archivio. Min o max possono
restare vuoti; se l'unita e indicata deve coincidere con quella dell'archivio.
Maiuscole e minuscole contano: mW e MW sono unita diverse. Senza unita compare
un avviso di verifica. Se il file diventa illeggibile, i limiti vengono rimossi
dal grafico e compare un errore: correggere il file o importarlo nuovamente.
I limiti sintetici vengono caricati automaticamente solo aprendo la demo.
Le righe che iniziano con # e quelle senza tag sono ignorate.
Un limite compare nel grafico quando e vicino ai valori letti, oppure
allontanando lo zoom con la rotella: la scala non viene allargata per
limiti lontani. Oltre il limite la zona e ombreggiata in rosso, con una
linea tratteggiata sul limite. Sono i limiti inseriti dall'utente, non le
soglie di allarme del sistema AMS. Il file resta sul PC: non condividerlo.

VISTE ABITUALI
Scegliere parametri, sorgente e periodo; in Viste abituali scrivere
un nome, per esempio Motore A - temperature, e premere Salva. Richiamando la vista
si caricano gli stessi parametri e la stessa durata sui dati piu recenti.
Una vista salvata con Tutto lo storico comprende anche i nuovi archivi
aggiunti successivamente. Le preferenze restano nella cartella del programma.

NUOVI EXPORT
Copiare gli export in days e weeks mantenendo i nomi originali
Day_...s3db e Week_from_..._to_...s3db. Terminata la copia, premere
Aggiorna, quindi selezionare i parametri o richiamare una vista.

PROBLEMI COMUNI
- Non si apre: verificare di avere estratto lo ZIP. Annotare l'eventuale
  messaggio. Se riguarda la grafica, rivolgersi al referente del PC.
- Nessun archivio: controllare Cartella dati... e la presenza degli .s3db.
- Parametro non trovato: scegliere Tutti gli impianti e Tutti.
- Nessuna lettura: controllare date e sorgente; verificare gli export disponibili.
- Attesa lunga: Annulla caricamento, ridurre la richiesta e ricaricare.
- Demo interrotta: scegliere una nuova cartella vuota con --demo --data-dir.
  Nessuna cartella esistente viene cancellata automaticamente.
- Archivio cambiato/illeggibile: attendere la fine della copia, premere
  Aggiorna e consultare Note archivio.
- Comando non visibile: ingrandire la finestra o scorrere il pannello laterale.
- Vista non salvata: usare una cartella del programma scrivibile dall'utente.
Se il problema rimane, comunicare al referente messaggio esatto, parametro,
periodo e sorgente selezionati.
