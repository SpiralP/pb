use pb::ProgressBar;
use std::collections::VecDeque;
use std::io::{Result, Stdout, Write};
use std::str::from_utf8;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::sync::RwLock;
use tty::move_cursor_up;

// StateMessage is the message format used to communicate
// between MultiBar and its bars
#[derive(Debug)]
enum StateMessage {
    Message(String, usize),
    BarDone(usize),
    CreateBar,
    Finish,
}

pub struct MultiBarPrinter<T: Write> {
    receiver: Receiver<StateMessage>,
    handle: T,

    lines: Arc<RwLock<VecDeque<String>>>,
    offset: usize,
}
impl<T: Write> MultiBarPrinter<T> {
    pub fn listen(&mut self) {
        loop {
            // receive message
            let msg = self.receiver.recv().unwrap();
            let mut lines = self.lines.write().unwrap();
            match msg {
                StateMessage::Message(message, level) => {
                    lines[level - self.offset] = message;

                    // and draw
                    let mut out = String::new();
                    let mut first = true;
                    for l in lines.iter() {
                        if first {
                            out += &move_cursor_up(lines.len());
                            first = false;
                        }
                        out.push_str(&format!("\r{}\n", l));
                    }
                    printfl!(self.handle, "{}", out);
                }
                StateMessage::BarDone(level) => {
                    lines[level - self.offset] = "".to_owned(); // mark line as useless

                    // pop the first line if it is useless
                    loop {
                        if lines.get(0).map(|line| line == "").unwrap_or(false) {
                            self.offset += 1;
                            lines.pop_front();
                        } else {
                            break;
                        }
                    }
                }
                StateMessage::CreateBar => {
                    printfl!(self.handle, "\n");
                }
                StateMessage::Finish => {
                    break;
                }
            }
        }
    }
}

pub struct MultiBar<T: Write> {
    lines: Arc<RwLock<VecDeque<String>>>,

    sender: Sender<StateMessage>,

    printer: Option<MultiBarPrinter<T>>,
}

impl MultiBar<Stdout> {
    /// Create a new MultiBar with stdout as a writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::thread;
    /// use pbr::MultiBar;
    /// use std::time::Duration;
    ///
    /// let mut mb = MultiBar::new();
    /// mb.println("Application header:");
    ///
    /// # let count = 250;
    /// let mut p1 = mb.create_bar(count);
    /// let _ = thread::spawn(move || {
    ///     for _ in 0..count {
    ///         p1.inc();
    ///         thread::sleep(Duration::from_millis(100));
    ///     }
    ///     // notify the multibar that this bar finished.
    ///     p1.finish();
    /// });
    ///
    /// mb.println("add a separator between the two bars");
    ///
    /// let mut p2 = mb.create_bar(count * 2);
    /// let _ = thread::spawn(move || {
    ///     for _ in 0..count * 2 {
    ///         p2.inc();
    ///         thread::sleep(Duration::from_millis(100));
    ///     }
    ///     // notify the multibar that this bar finished.
    ///     p2.finish();
    /// });
    ///
    /// // start listen to all bars changes.
    /// // this is a blocking operation, until all bars will finish.
    /// // to ignore blocking, you can run it in a different thread.
    /// mb.listen();
    /// ```
    pub fn new() -> MultiBar<Stdout> {
        MultiBar::on(::std::io::stdout())
    }
}

impl<T: Write> MultiBar<T> {
    /// Create a new MultiBar with an arbitrary writer.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    /// use std::io::stderr;
    ///
    /// let mut mb = MultiBar::on(stderr());
    /// // ...
    /// // see full example in `MultiBar::new`
    /// // ...
    /// ```
    pub fn on(handle: T) -> MultiBar<T> {
        let (sender, receiver) = mpsc::channel();
        let lines = Arc::new(RwLock::new(VecDeque::new()));

        MultiBar {
            lines: lines.clone(),
            sender,
            printer: Some(MultiBarPrinter {
                receiver,
                offset: 0,
                lines,
                handle,
            }),
        }
    }

    /// println used to add text lines between the bars.
    /// for example: you could add a header to your application,
    /// or text separators between bars.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    /// mb.println("Application header:");
    ///
    /// # let count = 250;
    /// let mut p1 = mb.create_bar(count);
    /// // ...
    ///
    /// mb.println("Text line between bar1 and bar2");
    ///
    /// let mut p2 = mb.create_bar(count);
    /// // ...
    ///
    /// mb.println("Text line between bar2 and bar3");
    ///
    /// // ...
    /// // ...
    /// mb.listen();
    /// ```
    pub fn println(&mut self, s: &str) {
        self.lines.write().unwrap().push_back(s.to_owned());
    }

    /// create_bar creates new `ProgressBar` with `Pipe` as the writer.
    ///
    /// The ordering of the method calls is important. it means that in
    /// the first call, you get a progress bar in level 1, in the 2nd call,
    /// you get a progress bar in level 2, and so on.
    ///
    /// ProgressBar that finish its work, must call `finish()` (or `finish_print`)
    /// to notify the `MultiBar` about it.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    /// # let (count1, count2, count3) = (250, 62500, 15625000);
    ///
    /// // progress bar in level 1
    /// let mut p1 = mb.create_bar(count1);
    /// // ...
    ///
    /// // progress bar in level 2
    /// let mut p2 = mb.create_bar(count2);
    /// // ...
    ///
    /// // progress bar in level 3
    /// let mut p3 = mb.create_bar(count3);
    ///
    /// // ...
    /// mb.listen();
    /// ```
    pub fn create_bar(&mut self, total: u64) -> ProgressBar<Pipe> {
        self.println("");
        self.sender.send(StateMessage::CreateBar).unwrap(); // just prints a newline

        let mut p = ProgressBar::on(
            Pipe {
                level: self.lines.read().unwrap().len() - 1,
                sender: self.sender.clone(),
            },
            total,
        );
        p.is_multibar = true;
        p.add(0);
        p
    }

    /// listen start listen to all bars changes.
    ///
    /// `ProgressBar` that finish its work, must call `finish()` (or `finish_print`)
    /// to notify the `MultiBar` about it.
    ///
    /// This is a blocking operation and blocks until all bars will
    /// finish.
    /// To ignore blocking, you can run it in a different thread.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::thread;
    /// use pbr::MultiBar;
    ///
    /// let mut mb = MultiBar::new();
    ///
    /// // ...
    /// // create some bars here
    /// // ...
    ///
    /// thread::spawn(move || {
    ///     mb.listen();
    ///     println!("all bars done!");
    /// });
    ///
    /// // ...
    /// ```
    pub fn listen(&mut self) {
        self.get_printer().listen();
    }

    /// Get a MultiBarPrinter to allow adding more ProgressBars while listening
    pub fn get_printer(&mut self) -> MultiBarPrinter<T> {
        self.printer.take().unwrap()
    }

    /// Finish and stop printing
    pub fn finish(&mut self) {
        self.sender.send(StateMessage::Finish).unwrap();
    }
}

impl Default for MultiBar<Stdout> {
    fn default() -> Self {
        Self::new()
    }
}

// impl<T: Write> Clone for MultiBar<T> {
//     fn clone(&self) -> Self {
//         MultiBar {
//             nbars: self.nbars, // maybe make these atomic
//             nlines: self.nlines,
//             sender: self.sender.clone(),
//             printer: None,
//         }
//     }
// }

impl<T: Write> Drop for MultiBar<T> {
    fn drop(&mut self) {
        self.sender.send(StateMessage::Finish).unwrap();
    }
}

pub struct Pipe {
    level: usize,
    sender: Sender<StateMessage>,
}

impl Write for Pipe {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let s = from_utf8(buf).unwrap().to_owned();

        // finish method emits empty string
        let msg = if s == "" {
            StateMessage::BarDone(self.level)
        } else {
            StateMessage::Message(s, self.level)
        };

        self.sender.send(msg).unwrap();

        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}
