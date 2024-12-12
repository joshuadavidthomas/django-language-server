pub mod v1 {
    pub mod messages {
        include!(concat!(env!("OUT_DIR"), "/djls.v1.messages.rs"));
    }

    pub mod check {
        include!(concat!(env!("OUT_DIR"), "/djls.v1.check.rs"));
    }

    pub mod django {
        include!(concat!(env!("OUT_DIR"), "/djls.v1.django.rs"));
    }

    pub mod python {
        include!(concat!(env!("OUT_DIR"), "/djls.v1.python.rs"));
    }
}
