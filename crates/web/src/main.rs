mod api;
mod units;
mod components;
mod pages;

use leptos::prelude::*;
use leptos_router::components::{ParentRoute, Route, Router, Routes};
use leptos_router::path;

use crate::components::layout::AppLayout;
use crate::pages::cars::{CarDetailPage, CarsPage};
use crate::pages::dashboard::DashboardPage;
use crate::pages::settings::SettingsPage;
use crate::pages::landing::LandingPage;
use crate::pages::login::LoginPage;
use crate::pages::not_found::NotFoundPage;
use crate::pages::routes::{RouteCorridorPage, RoutesPage};
use crate::pages::trips::{TripDetailPage, TripsPage};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| {
        view! {
            <Router>
                <Routes fallback=|| view! { <NotFoundPage/> }>
                    <Route path=path!("/") view=LandingPage/>
                    <Route path=path!("/login") view=LoginPage/>
                    // Keep the shell mounted across authenticated pages so the Google
                    // avatar is not re-requested on every navigation (can trigger 429).
                    <ParentRoute path=path!("/app") view=AppLayout>
                        <Route path=path!("") view=DashboardPage/>
                        <Route path=path!("cars") view=CarsPage/>
                        <Route path=path!("cars/:id") view=CarDetailPage/>
                        <Route path=path!("trips") view=TripsPage/>
                        <Route path=path!("trips/:id") view=TripDetailPage/>
                        <Route path=path!("routes") view=RoutesPage/>
                        <Route path=path!("routes/:id") view=RouteCorridorPage/>
                        <Route path=path!("settings") view=SettingsPage/>
                    </ParentRoute>
                </Routes>
            </Router>
        }
    });
}
