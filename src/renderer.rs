use crate::util::{data_export::*, ComplexFixed, ComplexArbitrary, PixelData, complex_extended::ComplexExtended, float_extended::FloatExtended};
use crate::math::{SeriesApproximation, Perturbation};

use std::time::Instant;
use std::cmp::{min, max};
use std::f64::consts::LOG2_10;

use rand::seq::SliceRandom;
use rayon::prelude::*;
use config::Config;

pub struct FractalRenderer {
    image_width: usize,
    image_height: usize,
    aspect: f64,
    zoom: FloatExtended,
    center_location: ComplexArbitrary,
    maximum_iteration: usize,
    approximation_order: usize,
    glitch_tolerance: f64,
    data_export: DataExport,
}

impl FractalRenderer {
    pub fn new(settings: Config) -> Self {
        let image_width = settings.get_int("image_width").unwrap() as usize;
        let image_height = settings.get_int("image_height").unwrap() as usize;
        let maximum_iteration = settings.get_int("iterations").unwrap() as usize;
        let initial_zoom = settings.get_str("zoom").unwrap();
        let center_real = settings.get_str("real").unwrap();
        let center_imag = settings.get_str("imag").unwrap();
        let approximation_order = 0;
        let glitch_tolerance = 0.01;
        let display_glitches = false;

        let aspect = image_width as f64 / image_height as f64;
        let temp: Vec<&str> = initial_zoom.split('E').collect();
        let zoom = FloatExtended::new(temp[0].parse::<f64>().unwrap() * 2.0_f64.powf((temp[1].parse::<f64>().unwrap() * LOG2_10).fract()), (temp[1].parse::<f64>().unwrap() * LOG2_10).floor() as i32);

        let delta_pixel =  (-2.0 * (4.0 / image_height as f64 - 2.0) / zoom) / image_height as f64;
        let radius = delta_pixel * image_width as f64;
        let precision = max(64, -radius.exponent + 64);

        let center_location = ComplexArbitrary::with_val(
            precision as u32,
            ComplexArbitrary::parse("(".to_owned() + &center_real + "," + &center_imag + ")").expect("Location is not valid!"));

        let auto_approximation = if approximation_order == 0 {
            let auto = (((image_width * image_height) as f64).log(1e6).powf(6.619) * 16.0f64) as usize;
            min(max(auto, 3), 64)
        } else {
            approximation_order
        };

        FractalRenderer {
            image_width,
            image_height,
            aspect,
            zoom,
            center_location,
            maximum_iteration,
            approximation_order: auto_approximation,
            glitch_tolerance,
            data_export: DataExport::new(image_width, image_height, display_glitches, DataType::BOTH)
        }
    }

    pub fn render(&mut self, _filename: String) {
        let delta_pixel =  (-2.0 * (4.0 / self.image_height as f64 - 2.0) / self.zoom.mantissa) / self.image_height as f64;

        // this should be the delta relative to the image, without the big zoom factor applied.
        let delta_top_left = ComplexFixed::new((4.0 / self.image_width as f64 - 2.0) / self.zoom.mantissa * self.aspect as f64, (4.0 / self.image_height as f64 - 2.0) / self.zoom.mantissa);

        let time = Instant::now();

        println!("Zoom: {}", self.zoom);

        let delta_pixel_extended = FloatExtended::new(delta_pixel, -self.zoom.exponent);

        // Series approximation currently has some overskipping issues
        // this can be resolved by root finding and adding new probe points
        let mut series_approximation = SeriesApproximation::new(
            self.center_location.clone(),
            self.approximation_order,
            self.maximum_iteration,
            delta_pixel_extended * delta_pixel_extended,
            ComplexExtended::new(delta_top_left, -self.zoom.exponent),
        );

        series_approximation.run();

        println!("{:<14}{:>6} ms", "Approximation", time.elapsed().as_millis());
        println!("{:<16}{:>6} (order {})", "Skipped", series_approximation.current_iteration, series_approximation.order);

        let time = Instant::now();

        let mut reference = series_approximation.get_reference(ComplexExtended::new2(0.0, 0.0, 0));
        reference.run();

        println!("{:<14}{:>6} ms (precision {}, iterations {})", "Reference", time.elapsed().as_millis(), self.center_location.prec().0, reference.current_iteration);

        let time = Instant::now();

        let mut pixel_data = (0..(self.image_width * self.image_height)).into_par_iter()
            .map(|index| {
                let i = index % self.image_width;
                let j = index / self.image_width;
                let element = ComplexFixed::new(i as f64 * delta_pixel + delta_top_left.re, j as f64 * delta_pixel + delta_top_left.im);
                let point_delta = ComplexExtended::new(element, -self.zoom.exponent);
                let new_delta = series_approximation.evaluate(point_delta);

                PixelData {
                    image_x: i,
                    image_y: j,
                    iteration: reference.start_iteration,
                    delta_centre: point_delta,
                    delta_reference: point_delta,
                    delta_start: new_delta,
                    delta_current: new_delta,
                    derivative_current: ComplexFixed::new(1.0, 0.0),
                    glitched: false,
                    escaped: false
                }
            }).collect::<Vec<PixelData>>();

        println!("{:<14}{:>6} ms", "Packing", time.elapsed().as_millis());

        let time = Instant::now();
        Perturbation::iterate(&mut pixel_data, &reference, reference.current_iteration);
        println!("{:<14}{:>6} ms", "Iteration", time.elapsed().as_millis());

        let time = Instant::now();
        self.data_export.export_pixels(&pixel_data, self.maximum_iteration, &reference);
        println!("{:<14}{:>6} ms", "Coloring", time.elapsed().as_millis());

        let time = Instant::now();

        // Remove all non-glitched points from the remaining points
        pixel_data.retain(|packet| {
            packet.glitched
        });

        while pixel_data.len() as f64 > 0.01 * self.glitch_tolerance * (self.image_width * self.image_height) as f64 {
            // delta_c is the difference from the next reference from the previous one
            let delta_c = pixel_data.choose(&mut rand::thread_rng()).unwrap().clone();
            let element = ComplexFixed::new(delta_c.image_x as f64 * delta_pixel + delta_top_left.re, delta_c.image_y as f64 * delta_pixel + delta_top_left.im);

            let reference_wrt_sa = ComplexExtended::new(element, -self.zoom.exponent);

            let delta_z = series_approximation.evaluate(reference_wrt_sa);

            let mut r = series_approximation.get_reference(reference_wrt_sa);
            r.run();

            // this can be made faster, without having to do the series approximation again
            // this is done by storing more data in pixeldata2
            pixel_data.par_iter_mut()
                .for_each(|pixel| {
                    pixel.iteration = reference.start_iteration;
                    pixel.glitched = false;
                    pixel.delta_current = pixel.delta_start - delta_z;
                    pixel.delta_reference = pixel.delta_centre - reference_wrt_sa;
                        // might not need the evaluate here as if we store it separately, there is no need
                        // data.derivative_current = ComplexFixed::new(1.0, 0.0);
                });

            Perturbation::iterate(&mut pixel_data, &r, r.current_iteration);

            self.data_export.export_pixels(&pixel_data, self.maximum_iteration, &r);

            // Remove all non-glitched points from the remaining points
            pixel_data.retain(|packet| {
                packet.glitched
            });
        }

        println!("{:<14}{:>6} ms (remaining {})", "Fixing", time.elapsed().as_millis(), pixel_data.len());
        
        let time = Instant::now();
        self.data_export.save();
        println!("{:<14}{:>6} ms", "Saving", time.elapsed().as_millis());
        return;
    }

    pub fn render_sequence(&mut self, scale_factor: f64) {
        let mut count = 0;
        while self.zoom.to_float() > 1.0 {
            self.render(format!("output/keyframe_{:08}.jpg", count));
            self.zoom.mantissa /= scale_factor;
            self.zoom.reduce();
            count += 1;
        }
    }
}