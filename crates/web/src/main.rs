mod api;
mod components;
mod pages;

use leptos::prelude::*;
use leptos_router::components::{ParentRoute, Route, Router, Routes};
use leptos_router::path;

use crate::components::layout::AppLayout;
use crate::pages::cars::{CarDetailPage, CarsPage};
use crate::pages::dashboard::DashboardPage;
use crate::pages::login::LoginPage;
use crate::pages::trips::{TripDetailPage, TripsPage};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| {
        view! {
            <Router>
                <Routes fallback=|| view! { <p class="muted">"Not found"</p> }>
                    <Route path=path!("/login") view=LoginPage/>
                    // Keep the shell mounted across authenticated pages so the Google
                    // avatar is not re-requested on every navigation (can trigger 429).
                    <ParentRoute path=path!("/") view=AppLayout>
                        <Route path=path!("") view=DashboardPage/>
                        <Route path=path!("cars") view=CarsPage/>
                        <Route path=path!("cars/:id") view=CarDetailPage/>
                        <Route path=path!("trips") view=TripsPage/>
                        <Route path=path!("trips/:id") view=TripDetailPage/>
                    </ParentRoute>
                </Routes>
            </Router>
        }
    });
}
