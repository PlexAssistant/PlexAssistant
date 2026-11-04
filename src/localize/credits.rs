//! Credits for one film or one episode, asked for in Chinese.
//!
//! The key comes from the caller. A request that does not come back as
//! credits is reported as nothing, so a scan can skip the item and keep
//! going. A name with no Chinese form comes back as the original name.

use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;

use super::{Credit, CreditTarget, Credits};

const API: &str = "https://api.themoviedb.org/3";
const LANGUAGE: &str = "zh-CN";

/// A request gives up on its own after this long, so a connection that
/// never answers cannot hold the scan's item loop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// One person on a credits list. Fields this pass does not read are ignored.
#[derive(Deserialize)]
struct Person {
    name: String,
    original_name: String,
    #[serde(default)]
    job: String,
    #[serde(default)]
    department: String,
}

/// The credits body. A film and an episode share `cast` and `crew`; an
/// episode also lists `guest_stars`, which belong with the cast.
#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    cast: Vec<Person>,
    #[serde(default)]
    crew: Vec<Person>,
    #[serde(default)]
    guest_stars: Vec<Person>,
}

pub(super) async fn fetch(key: &str, target: CreditTarget) -> Option<Credits> {
    let path = match target {
        CreditTarget::Movie(id) => format!("/movie/{id}/credits"),
        CreditTarget::Show(id) => format!("/tv/{id}/credits"),
        CreditTarget::Episode {
            id,
            season,
            episode,
        } => {
            format!("/tv/{id}/season/{season}/episode/{episode}/credits")
        }
    };
    let url = format!(
        "{API}{path}?api_key={}&language={LANGUAGE}",
        url_encode(key)
    );
    fetch_url(client(), &url).await
}

/// The credits at `url`, or nothing when the request does not come back as
/// credits. The caller's client decides how long to wait.
async fn fetch_url(client: &reqwest::Client, url: &str) -> Option<Credits> {
    let bytes = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    let body: Body = serde_json::from_slice(&bytes).ok()?;
    Some(read(body))
}

/// One client for the whole scan, so the connection is reused across items.
/// A request that never comes back gives up on its own.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("a client with a timeout always builds")
    })
}

/// The key goes in the query string, so anything that would change the
/// request is escaped. A normal key has nothing to escape.
fn url_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Directors, writers and cast, in list order. A person listed twice under
/// the same heading is kept once. Writing jobs other than the screenplay
/// itself, such as the novel a film was drawn from, are not writers.
fn read(body: Body) -> Credits {
    let mut directors = Vec::new();
    let mut writers = Vec::new();
    for person in body.crew {
        if person.job == "Director" {
            push_once(&mut directors, credit(person));
        } else if person.department == "Writing" && is_writer(&person.job) {
            push_once(&mut writers, credit(person));
        }
    }
    let mut cast = Vec::new();
    for person in body.cast.into_iter().chain(body.guest_stars) {
        push_once(&mut cast, credit(person));
    }
    Credits {
        directors,
        writers,
        cast,
    }
}

fn credit(person: Person) -> Credit {
    Credit {
        name: person.name,
        original: person.original_name,
    }
}

fn push_once(list: &mut Vec<Credit>, credit: Credit) {
    if credit.name.is_empty() {
        return;
    }
    if list.iter().any(|have| have.original == credit.original) {
        return;
    }
    list.push(credit);
}

/// A writing credit Plex files under writers. A novel, a comic or a story
/// the script was drawn from is not one.
fn is_writer(job: &str) -> bool {
    matches!(
        job,
        "Writer" | "Screenplay" | "Teleplay" | "Story" | "Screenstory"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(name: &str, original: &str, job: &str, department: &str) -> Person {
        Person {
            name: name.into(),
            original_name: original.into(),
            job: job.into(),
            department: department.into(),
        }
    }

    #[test]
    fn a_film_splits_directors_writers_and_cast() {
        let credits = read(Body {
            cast: vec![person("甲演员", "Ada Actor", "", "")],
            crew: vec![
                person("甲导演", "Ada Director", "Director", "Directing"),
                person("甲编剧", "Ada Writer", "Screenplay", "Writing"),
                person("甲原著", "Ada Novelist", "Novel", "Writing"),
            ],
            guest_stars: vec![],
        });
        assert_eq!(credits.directors[0].name, "甲导演");
        assert_eq!(credits.writers.len(), 1);
        assert_eq!(credits.writers[0].name, "甲编剧");
        assert_eq!(credits.cast[0].original, "Ada Actor");
    }

    #[test]
    fn an_episode_keeps_guests_with_the_cast_and_drops_a_repeat() {
        let credits = read(Body {
            cast: vec![person("甲演员", "Ada Actor", "", "")],
            crew: vec![person("甲导演", "Ada Director", "Director", "Directing")],
            guest_stars: vec![
                person("甲演员", "Ada Actor", "", ""),
                person("Guest Person", "Guest Person", "", ""),
            ],
        });
        assert_eq!(credits.directors[0].original, "Ada Director");
        assert_eq!(
            credits
                .cast
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["甲演员", "Guest Person"]
        );
    }

    #[test]
    fn a_name_with_no_chinese_form_stays() {
        let credits = read(Body {
            cast: vec![],
            crew: vec![person("Ada Writer", "Ada Writer", "Writer", "Writing")],
            guest_stars: vec![],
        });
        assert_eq!(credits.writers[0].name, "Ada Writer");
    }

    #[tokio::test]
    async fn a_request_that_never_comes_back_gives_up() {
        // a socket that is listening but never accepts: the connect lands
        // in the backlog and the request is written, but no response ever
        // arrives
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let url = format!("http://{addr}/3/movie/1/credits?api_key=k&language=zh-CN");
        let started = std::time::Instant::now();
        assert!(fetch_url(&client, &url).await.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
